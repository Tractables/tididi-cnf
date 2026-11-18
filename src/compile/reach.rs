//! Widths counted over the nodes a diagram's output reaches.
//!
//! An operation may leave nodes behind that nothing names until a prune drops
//! them, and how many depends on the library version. The batch stop's width
//! cap reads its width here, so that number moves no width stop.

use tididi::diagram::{ChildDecoder, ChildRef, EncodedChildRef, NodeIdx, Tdd, ValueRef, ZERO};
use tididi::vtree::VtreeIdx;

/// Mark rows reused across calls of [`ReachScratch::width_on_spine`]: one flat row
/// of slots, and each spine level's offset into it.
#[derive(Default)]
pub(super) struct ReachScratch {
    marks: Vec<bool>,
    offset: Vec<usize>,
    order: Vec<VtreeIdx>,
}

impl ReachScratch {
    /// The largest count of reached nodes on one level of `spine`, an
    /// ancestor-closed set of levels, at a cost in proportion to their slots.
    /// Levels off `spine` are not read; a node there reaches nothing counted.
    pub(super) fn width_on_spine(&mut self, tdd: &Tdd, spine: &[VtreeIdx]) -> usize {
        if tdd.is_zero() {
            return 0;
        }
        let vtree = tdd.vtree();
        self.order.clear();
        self.order.extend(spine.iter().copied().filter(|&t| !vtree.node(t).is_leaf()));
        self.order.sort_unstable_by_key(|&t| std::cmp::Reverse(vtree.topo_pos(t)));
        if self.offset.len() < vtree.num_nodes() {
            self.offset.resize(vtree.num_nodes(), usize::MAX);
        }
        let mut total = 0;
        for &t in &self.order {
            self.offset[t.idx()] = total;
            total += tdd.level(t).slot_count();
        }
        self.marks.clear();
        self.marks.resize(total, false);
        let out = tdd.output();
        if let Some(&base) = self.offset.get(out.vtree.idx())
            && base != usize::MAX
        {
            self.marks[base + out.local.idx()] = true;
        }
        let mut width = 0;
        for &t in &self.order {
            let base = self.offset[t.idx()];
            let level = tdd.level(t);
            if level.is_marginal() {
                width = width.max(self.marks[base..base + level.slot_count()].iter().filter(|&&r| r).count());
                continue;
            }
            let (left, right) = vtree.children(t);
            let sides = [(left, tdd.level(left).child_decoder()), (right, tdd.level(right).child_decoder())];
            let mut here = 0;
            for (i, node) in level.nodes().iter().enumerate() {
                if !self.marks[base + i] {
                    continue;
                }
                here += 1;
                for pair in level.pairs_iter_of(&node) {
                    for (&(child, decoder), side) in sides.iter().zip([pair.left, pair.right]) {
                        let child_base = self.offset[child.idx()];
                        if child_base != usize::MAX
                            && let Some(s) = child_slot(decoder, side)
                        {
                            self.marks[child_base + s] = true;
                        }
                    }
                }
            }
            width = width.max(here);
        }
        for &t in &self.order {
            self.offset[t.idx()] = usize::MAX;
        }
        width
    }
}

/// The slot of the child level that `side` names; `None` for `ZERO` and for
/// an inline count.
fn child_slot(decoder: ChildDecoder, side: EncodedChildRef) -> Option<usize> {
    if side == ZERO.into() {
        return None;
    }
    match decoder.child(side) {
        ChildRef::Node(NodeIdx(i)) | ChildRef::Value(ValueRef::Slot(i)) => Some(i as usize),
        ChildRef::Value(ValueRef::Inline(_)) => None,
    }
}
