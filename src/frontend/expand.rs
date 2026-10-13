//! Reconstruct a compiled Boolean function in its original variable space.
//!
//! Vitri's compile record partitions originals into signed alias classes,
//! constants and free variables. A reduced leaf is replaced by the conjunction
//! for each value of its whole signed class, over a vtree whose leaf is
//! replaced by the adjacent subtree of the class's originals; constants and
//! free variables are then grafted on above the root. This reconstructs the
//! function itself, so no count lift is owed after expansion.

use std::collections::HashMap;
use std::sync::Arc;

use tididi::diagram::{
    ChildPair, ChildRef, EncodedChildRef, Literal, NodeIdx, Tdd, TddBuilder, TddLevel, TddNodeId, LEAF_WIDTH,
    NEG_LEAF_IDX, ONE_LEAF_IDX, POS_LEAF_IDX, ZERO,
};
use tididi::restructure::GraftError;
use tididi::vtree::{VarId, Vtree, VtreeIdx, VtreeNode};
use tididi::{Engine, OperationError};

use super::record::OriginalExpansion;

/// Expand one reduced TDD into the original variables: rebuild it over the
/// vtree it ends on with every leaf replaced by its class, which the compile
/// may have rotated away from the vtree it started on, then graft the
/// constants and free variables on.
///
/// A negative singleton class is a real operation (`Pos`/`Neg` swap), while a
/// positive singleton remains an index-preserving relabel.
pub(crate) fn expand_to_originals(
    eng: &Engine,
    tdd: &Tdd,
    expansion: &OriginalExpansion,
) -> Result<Tdd, OperationError> {
    let class_vtree = expand_class_vtree(tdd.vtree(), expansion);
    let class_expanded = expand_tdd_classes(eng, tdd, &class_vtree, expansion)?;
    if expansion.constants.is_empty() && expansion.free.is_empty() {
        return Ok(class_expanded);
    }
    let context = Arc::clone(class_expanded.context());
    let mut parts = vec![class_expanded];
    for &(var, value) in &expansion.constants {
        parts.push(eng.literal(&context.bind(Vtree::leaf(var)), Literal::new(var, value))?);
    }
    // These operations belong to the same bounded batch as compilation and
    // class rebuilding. Cancellation is a refusal, not a malformed map.
    eng.graft(parts, &expansion.free).map_err(|error| match error {
        GraftError::Operation(error) => error,
        error => panic!("a total original map names each original once: {error}"),
    })
}

/// Replace every leaf of a reduced compiler vtree with a balanced subtree over
/// its original-variable class.
///
/// The vtree has one leaf per reduced variable: vitri builds it that way,
/// compiling only rotates it or grafts over the same variables, and a vtree
/// never puts one variable on two leaves. So every class is placed once, and
/// every leaf names a class: `expansion` has one class per reduced variable.
fn expand_class_vtree(reduced: &Vtree, expansion: &OriginalExpansion) -> Arc<Vtree> {
    let mut nodes = Vec::new();
    let mut idx_map = vec![None; reduced.num_nodes()];

    for (old_idx, reduced_var) in reduced.leaf_bottomup() {
        let class = &expansion.classes[reduced_var.idx()];
        let originals: Vec<VarId> = class.iter().map(|&(original, _)| original).collect();
        let new_root = Vtree::build_balanced_recursive(&originals, &mut nodes);
        idx_map[old_idx.idx()] = Some(new_root);
    }
    for (old_idx, old_left, old_right) in reduced.internal_bottomup() {
        let new_left = idx_map[old_left.idx()].expect("child rebuilt before parent");
        let new_right = idx_map[old_right.idx()].expect("child rebuilt before parent");
        let new_idx = VtreeIdx(nodes.len() as u32);
        nodes.push(VtreeNode::Internal {
            left: new_left,
            right: new_right,
            parent: None,
        });
        idx_map[old_idx.idx()] = Some(new_idx);
    }

    let class_root = idx_map[reduced.root().idx()].expect("the reduced root was rebuilt");
    reduced.context().bind(
        Vtree::from_nodes(nodes, class_root, expansion.original_num_vars)
            .expect("the class expansion rebuilds one tree"),
    )
}

/// Materialize the conjunction described by a compile record with no reduced
/// variables. Every original is then either constant or free.
///
/// Vitri reports a run fully resolved exactly when the reduced formula has no
/// variables, so there is no class, and it refuses a formula with none before
/// it preprocesses, so the balanced vtree over the originals has a leaf.
pub(crate) fn fully_resolved_tdd(eng: &Engine, expansion: &OriginalExpansion) -> Result<Tdd, OperationError> {
    let vtree = Arc::new(Vtree::balanced(expansion.original_num_vars));
    // With no class left, `OriginalExpansion::from_record` put every original
    // among the constants or the free variables. The free variables carry no
    // literal: a cube says nothing about them, which is exactly the One leaf
    // this expansion places for them.
    let literals = expansion
        .constants
        .iter()
        .map(|&(var, value)| Literal::new(var, value));
    eng.cube(&vtree, literals)
}

fn structural_leaf_index(level: &TddLevel, side: EncodedChildRef) -> usize {
    match level.child_decoder().child(side) {
        ChildRef::Node(node) => node.idx(),
        ChildRef::Value(_) => panic!("class expansion requires structural leaves"),
    }
}

fn expand_tdd_classes(
    eng: &Engine,
    tdd: &Tdd,
    class_vtree: &Arc<Vtree>,
    expansion: &OriginalExpansion,
) -> Result<Tdd, OperationError> {
    let reduced_vtree = &tdd.vtree();
    let idx_map = build_vtree_idx_map(reduced_vtree, class_vtree);
    let mut out = Tdd::builder(eng, class_vtree)?;

    // Only materialize leaf mimics that are referenced from above. Besides
    // keeping the no-unreachable-nodes invariant, this handles a reduced TDD
    // whose output is itself a leaf without padding its new class root.
    let mut leaf_used = HashMap::<VtreeIdx, [bool; LEAF_WIDTH]>::new();
    for (reduced_idx, reduced_var) in reduced_vtree.leaf_bottomup() {
        let class = &expansion.classes[reduced_var.idx()];
        if class.len() > 1 || !class[0].1 {
            leaf_used.insert(reduced_idx, [false; LEAF_WIDTH]);
        }
    }
    if let Some(used) = leaf_used.get_mut(&tdd.output().vtree) {
        let local = tdd.output().local.idx();
        if local < LEAF_WIDTH {
            used[local] = true;
        }
    }
    for (reduced_idx, left, right) in reduced_vtree.internal_bottomup() {
        let level = &tdd.levels()[reduced_idx.idx()];
        for node in level.nodes() {
            for pair in level.pairs_iter_of(node) {
                for (child, side) in [(left, pair.left), (right, pair.right)] {
                    if let Some(used) = leaf_used.get_mut(&child)
                        && side != ZERO.into()
                    {
                        used[structural_leaf_index(tdd.level(child), side)] = true;
                    }
                }
            }
        }
    }

    let mut leaf_remap = HashMap::<VtreeIdx, [Option<NodeIdx>; LEAF_WIDTH]>::new();
    for (reduced_idx, reduced_var) in reduced_vtree.leaf_bottomup() {
        if let Some(&used) = leaf_used.get(&reduced_idx) {
            // The reduced leaf becomes its signed original-variable class, the
            // leaves under its expanded index. Only the positions referenced
            // from above, which `used` marks, are built; the rest remap to
            // `None` and appear in no parent pair ref.
            let same_polarity: HashMap<VarId, bool> =
                expansion.classes[reduced_var.idx()].iter().copied().collect();
            let remap = build_subtree_internal_levels(
                eng,
                class_vtree,
                idx_map[reduced_idx.idx()],
                &same_polarity,
                used,
                &mut out,
            )?;
            leaf_remap.insert(reduced_idx, remap);
        }
    }

    for (reduced_idx, left, right) in reduced_vtree.internal_bottomup() {
        let expanded_idx = idx_map[reduced_idx.idx()];
        let reduced_level = &tdd.levels()[reduced_idx.idx()];
        let left_remap = leaf_remap.get(&left);
        let right_remap = leaf_remap.get(&right);

        if left_remap.is_none() && right_remap.is_none() {
            out.replace_level(eng, expanded_idx, tdd.level_view(reduced_idx))?;
        } else {
            // A reference into a class leaf goes to the mimic built for it;
            // zero and references into other levels are kept.
            let remap = |child: VtreeIdx, map: Option<&[Option<NodeIdx>; LEAF_WIDTH]>, side: EncodedChildRef| {
                match map {
                    Some(map) if side != ZERO.into() => map[structural_leaf_index(tdd.level(child), side)]
                        .expect("referenced class mimic was not built")
                        .into(),
                    _ => side,
                }
            };
            let mut pairs = Vec::new();
            for node in reduced_level.nodes() {
                pairs.clear();
                for pair in reduced_level.pairs_iter_of(node) {
                    pairs.push(ChildPair::new(
                        remap(left, left_remap, pair.left),
                        remap(right, right_remap, pair.right),
                    ));
                }
                out.push(eng, expanded_idx, &pairs)?;
            }
        }
    }

    let output_local = if tdd.output().local == ZERO {
        ZERO
    } else {
        leaf_remap
            .get(&tdd.output().vtree)
            .map_or(tdd.output().local, |map| {
                map[tdd.output().local.idx()].expect("output class mimic was not built")
            })
    };
    Ok(out
        .finish(TddNodeId {
            vtree: idx_map[reduced_vtree.root().idx()],
            local: output_local,
        })
        .expect("class expansion mirrors a well-formed diagram level for level"))
}

/// Map every node of `reduced_vtree` to its counterpart in `expanded_vtree`,
/// walking the two in structural lockstep from their roots. A reduced leaf
/// maps to whatever sits at the same position, either a leaf or the root of the
/// original-variable class inserted for that reduced leaf.
fn build_vtree_idx_map(reduced_vtree: &Vtree, expanded_vtree: &Vtree) -> Vec<VtreeIdx> {
    let mut map = vec![VtreeIdx(0); reduced_vtree.num_nodes()];
    let mut stack = vec![(reduced_vtree.root(), expanded_vtree.root())];
    while let Some((r_node, e_node)) = stack.pop() {
        map[r_node.idx()] = e_node;
        match (reduced_vtree.node(r_node), expanded_vtree.node(e_node)) {
            (VtreeNode::Leaf { .. }, _) => {}
            (
                VtreeNode::Internal {
                    left: rl,
                    right: rr,
                    ..
                },
                VtreeNode::Internal {
                    left: el,
                    right: er,
                    ..
                },
            ) => {
                stack.push((*rl, *el));
                stack.push((*rr, *er));
            }
            _ => unreachable!("reduced internal node maps to expanded leaf"),
        }
    }
    map
}

/// Build the internal levels of a class subtree, building only the mimics
/// `needed` marks.
///
/// `needed` and the returned remap are both indexed by the leaf encoding's
/// implicit `{One=0, Pos=1, Neg=2}` positions. The remap holds the new local
/// index of each built mimic, `None` for mimics not built; the caller uses it
/// to rewrite parent pair refs.
///
/// Propagation: a Pos mimic at this level requires Pos at both children; a
/// Neg mimic requires Neg at both; a One mimic (root only) requires both Pos
/// and Neg at both children. So child needs are
/// `[false, needed[POS] || needed[ONE], needed[NEG] || needed[ONE]]`.
///
/// At leaves, returns the implicit indices themselves, with Pos/Neg swapped
/// at a leaf whose variable `same_polarity` maps to `false`, that is, whose
/// polarity disagrees with the equivalence representative. Only the entries in
/// `needed` are guaranteed populated.
fn build_subtree_internal_levels(
    eng: &Engine,
    vtree: &Vtree,
    node: VtreeIdx,
    same_polarity: &HashMap<VarId, bool>,
    needed: [bool; LEAF_WIDTH],
    out: &mut TddBuilder,
) -> Result<[Option<NodeIdx>; LEAF_WIDTH], OperationError> {
    // Iterative post-order with a Begin/Combine work stack and a parallel
    // results stack. A recursive form overflows the default 8 MiB call stack on
    // linear-shaped vtrees with ≥10⁵ vars (one frame per spine level), and a
    // vtree-num_nodes-sized scratch buffer is too allocation-heavy for short
    // subtrees called many times. Both stacks here are O(subtree_size) heap
    // memory, so depth is unbounded and there is no per-call O(vtree)
    // allocation.
    enum Frame {
        Begin(VtreeIdx, [bool; LEAF_WIDTH]),
        Combine(VtreeIdx, [bool; LEAF_WIDTH]),
    }
    // The remap and `needed` slots: the leaf encoding's implicit indices.
    const ONE: usize = 0;
    const POS: usize = 1;
    const NEG: usize = 2;

    let mut work: Vec<Frame> = vec![Frame::Begin(node, needed)];
    let mut results: Vec<[Option<NodeIdx>; LEAF_WIDTH]> = Vec::new();

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Begin(idx, needed) => {
                if let VtreeNode::Leaf { var, .. } = vtree.node(idx) {
                    let same = *same_polarity
                        .get(var)
                        .expect("class-expanded subtree contains only members of this class");
                    let (pos, neg) = if same {
                        (POS_LEAF_IDX, NEG_LEAF_IDX)
                    } else {
                        (NEG_LEAF_IDX, POS_LEAF_IDX)
                    };
                    results.push([Some(ONE_LEAF_IDX), Some(pos), Some(neg)]);
                    continue;
                }
                let child_needed = [false, needed[POS] || needed[ONE], needed[NEG] || needed[ONE]];
                let (left, right) = vtree.children(idx);
                // Push Combine first so it runs after both children. Then push
                // right then left so left pops first; its result lands below
                // right's on the results stack.
                work.push(Frame::Combine(idx, needed));
                work.push(Frame::Begin(right, child_needed));
                work.push(Frame::Begin(left, child_needed));
            }
            Frame::Combine(idx, needed) => {
                let r_remap = results.pop().expect("missing right-child remap");
                let l_remap = results.pop().expect("missing left-child remap");

                let mut self_remap = [None; LEAF_WIDTH];

                if needed[POS] {
                    let pair = ChildPair::new(l_remap[POS].expect("Pos requires child Pos"), r_remap[POS].expect("Pos requires child Pos"));
                    self_remap[POS] = Some(out.push(eng, idx, &[pair])?);
                }
                if needed[NEG] {
                    let pair = ChildPair::new(l_remap[NEG].expect("Neg requires child Neg"), r_remap[NEG].expect("Neg requires child Neg"));
                    self_remap[NEG] = Some(out.push(eng, idx, &[pair])?);
                }
                if needed[ONE] {
                    let pos_pair = ChildPair::new(l_remap[POS].expect("One requires child Pos"), r_remap[POS].expect("One requires child Pos"));
                    let neg_pair = ChildPair::new(l_remap[NEG].expect("One requires child Neg"), r_remap[NEG].expect("One requires child Neg"));
                    self_remap[ONE] = Some(out.push(eng, idx, &[pos_pair, neg_pair])?);
                }

                results.push(self_remap);
            }
        }
    }

    debug_assert_eq!(
        results.len(),
        1,
        "build_subtree_internal_levels: results stack should hold exactly the root's remap"
    );
    Ok(results[0])
}

#[cfg(test)]
#[path = "tests/expand.rs"]
mod tests;
