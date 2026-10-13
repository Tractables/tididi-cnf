use tididi::diagram::{ChildDecoder, ChildRef};
use tididi::vtree::{Vtree, VtreeIdx};
use vitri::cnf::CnfFormula;

use super::*;
use crate::test_support::{clause, formula, sat_prune_ctx};

/// A false diagram is dead at its root without a probe.
#[test]
fn a_false_diagram_is_a_dead_root() {
    let eng = &Engine::new();
    let f = formula(2, vec![clause(&[1, 2])]);
    let mut ctx = sat_prune_ctx(&f);
    let vtree = std::sync::Arc::new(Vtree::balanced(2));
    assert!(matches!(ctx.probe_tdd(eng, &Tdd::zero(&vtree), 0), PruneOutcome::DeadRoot));
    assert_eq!(ctx.stats.passes_dead_root, 1);
}

/// The fixture of the inline-reference and spend-cap tests: a balanced(4)
/// TDD whose v4 level holds p = [(Pos, Inline(5))] (live) and
/// p2 = [(Neg, Inline(0))] (dead by its count) over a marginalized leaf
/// child with no overflow slots. The formula x1 ∧ x3 implies the
/// diagram, as the enumeration requires. A default probe yields Rewritten
/// with p2 pruned. Returns (formula, tdd, v4).
fn inline_marg_fixture() -> (CnfFormula, tididi::diagram::Tdd, VtreeIdx) {
    use std::sync::Arc;
    use tididi::diagram::{
        ChildPair, LeafLabel, NodeIdx, ValueRef, Tdd, TddNodeId,
    };

    // x1 ∧ x3 over the 4 vtree vars: every model satisfies the diagram.
    let f = formula(4, vec![clause(&[1]), clause(&[3])]);

    let vtree = Arc::new(Vtree::balanced(4));
    let root_idx = vtree.root();
    let (v4, v5) = vtree.children(root_idx);
    // Marginalize the right leaf child of v4. All its references are
    // inline, so the level has no overflow slots.
    let (_v_leaf0, v_marg) = vtree.children(v4);

    let eng = tididi::Engine::new();
    let mut b = Tdd::builder(&eng, &vtree).unwrap();
    let mut donor = eng.cube(&vtree, std::iter::empty::<i32>()).unwrap();
    eng.marginalize_levels(&mut donor, &[v_marg]).unwrap();
    assert_eq!(donor.level(v_marg).marginal_counts(), Some([].as_slice()));
    b.replace_level(&eng, v_marg, donor.level_view(v_marg)).unwrap();

    let pos = NodeIdx(LeafLabel::Pos as u32);
    let neg = NodeIdx(LeafLabel::Neg as u32);
    let one = NodeIdx(LeafLabel::One as u32);
    let live_inline = ValueRef::Inline(5).side().expect("the fixture reference fits its encoding");
    let dead_inline = ValueRef::Inline(0).side().expect("the fixture reference fits its encoding");

    // v4: p = [(Pos, Inline(5))] stays live; p2 = [(Neg, Inline(0))] is
    // dead-by-count (its only pair contribution is ⊥).
    let p = b.push(&eng, v4, &[ChildPair::new(pos, live_inline)]).unwrap();
    let p2 = b.push(&eng, v4, &[ChildPair::new(neg, dead_inline)]).unwrap();

    let s0 = b.push(&eng, v5, &[ChildPair::new(pos, one)]).unwrap();
    let s1 = b.push(&eng, v5, &[ChildPair::new(neg, one)]).unwrap();

    let root_node = b.push(&eng, root_idx, &[
        ChildPair::new(p, s0),
        ChildPair::new(p2, s1),
    ]).unwrap();

    let tdd = b
        .finish(TddNodeId { vtree: root_idx, local: root_node })
        .expect("the fixture seats its root on the node it just built");
    (f, tdd, v4)
}

/// A pass handles inline count references into a marginal child end to
/// end: the encoding resolves them (a count of 0 contributes nothing, a
/// positive count is ⊤), the probe kills the dead parent, and the rewrite
/// passes the surviving inline references through unchanged. A `probe_tdd`
/// that declined every pass on seeing them fails on the `Rewritten` match.
#[test]
fn inline_marg_refs_probe_and_rewrite_natively() {
    let eng = &Engine::new();
    use tididi::diagram::ValueRef;

    let (f, tdd, v4) = inline_marg_fixture();
    let mut ctx = sat_prune_ctx(&f);
    let size = tdd.pair_count();
    match ctx.probe_tdd(eng, &tdd, size) {
        PruneOutcome::Rewritten(new_tdd) => {
            assert!(
                ctx.stats.nodes_pruned >= 1,
                "p2 (inline count 0) must be pruned, nodes_pruned = {}",
                ctx.stats.nodes_pruned
            );
            // p survives alone at v4, its inline reference unchanged.
            let v4_lvl = &new_tdd.levels()[v4.idx()];
            assert_eq!(v4_lvl.nodes().len(), 1, "only p survives at v4");
            let pairs = v4_lvl.pairs_of_idx(0);
            assert_eq!(pairs.len(), 1);
            assert_eq!(
                ChildDecoder::marginal().child(pairs[0].right),
                ChildRef::Value(ValueRef::Inline(5)),
                "surviving inline ref must pass through the rewrite unchanged"
            );
            // The root keeps only the (p, s0) pair.
            let out = new_tdd.output();
            assert_eq!(
                new_tdd.levels()[out.vtree.idx()]
                    .pairs_of_idx(out.local.idx())
                    .len(),
                1,
                "root must drop the (p2, s1) pair"
            );
        }
        outcome => panic!("expected Rewritten, got {outcome:?}"),
    }
}

/// Obtain two positive slot references through checked construction and summation.
/// The counts exceed the inline range and differ, so marginalization retains both.
fn marginal_slot_donor(eng: &Engine) -> (tididi::Tdd, VtreeIdx) {
    use std::sync::Arc;
    use tididi::diagram::{ChildPair, Tdd, TddNodeId, ONE_LEAF_IDX, POS_LEAF_IDX, NEG_LEAF_IDX};

    let vtree = Arc::new(Vtree::balanced(64));
    let mut builder = Tdd::builder(eng, &vtree).unwrap();
    let mut labels = vec![[ONE_LEAF_IDX, POS_LEAF_IDX, NEG_LEAF_IDX]; vtree.num_nodes()];
    for level in vtree.bottomup() {
        if vtree.node(level).is_leaf() {
            continue;
        }
        let (left, right) = vtree.children(level);
        labels[level.idx()] = std::array::from_fn(|label| {
            builder.push(eng, level, &[ChildPair::new(labels[left.idx()][label], labels[right.idx()][0])]).unwrap()
        });
    }
    let root = vtree.root();
    let (left, source) = vtree.children(root);
    let output = builder.push(eng, root, &[
        ChildPair::new(labels[left.idx()][1], labels[source.idx()][0]),
        ChildPair::new(labels[left.idx()][2], labels[source.idx()][1]),
    ]).unwrap();
    let mut donor = builder.finish(TddNodeId { vtree: root, local: output }).unwrap();
    eng.marginalize_levels(&mut donor, &[source]).unwrap();
    let counts = donor.level(source).marginal_counts().unwrap();
    let mut actual = counts.to_vec();
    actual.sort_unstable();
    assert_eq!(actual, [1u128 << 31, 1u128 << 32]);
    (donor, source)
}

/// Marginalized boundary fixture: root = [(n1, s0), (n2, s0)] where
/// n1 = [(Pos, Slot(0))] and n2 = [(Neg, Slot(1))] at v4, the level over
/// {x1, x4} holding two positive, non-inline marginal counts, and
/// s0 = [(Pos, One)] at v5 (function x2). Φ is the DIMACS unit 3 (x2).
///
/// The distinct primes make the ⊤-erased boundary functions disjoint
/// (x1 vs ¬x1), so the partition certificate holds.
fn marg_boundary_fixture() -> (CnfFormula, tididi::diagram::Tdd) {
    use std::sync::Arc;
    use tididi::diagram::{
        ChildPair, LeafLabel, NodeIdx, ValueRef, Tdd, TddNodeId,
    };

    let f = formula(5, vec![clause(&[3])]);

    // The marginal boundary level must be internal: a count-marginal leaf
    // carries only its fixed label column, so the donor's two slots sit on a
    // node over {x1, x4} instead.
    let leaf = |v: u32| Vtree::leaf(tididi::vtree::VarId(v + 1));
    let vtree = Arc::new(
        Vtree::join(
            &Vtree::join(&leaf(0), &Vtree::join(&leaf(1), &leaf(4)).unwrap()).unwrap(),
            &Vtree::join(&leaf(2), &leaf(3)).unwrap(),
        )
        .unwrap(),
    );
    let root_idx = vtree.root();
    let (v4, v5) = vtree.children(root_idx);
    let (_leaf0, v_marg) = vtree.children(v4);

    let eng = tididi::Engine::new();
    let mut b = Tdd::builder(&eng, &vtree).unwrap();
    let (donor, source) = marginal_slot_donor(&eng);
    b.replace_level(&eng, v_marg, donor.level_view(source)).unwrap();

    let pos = NodeIdx(LeafLabel::Pos as u32);
    let neg = NodeIdx(LeafLabel::Neg as u32);
    let one = NodeIdx(LeafLabel::One as u32);
    let slot0 = ValueRef::Slot(0).side().expect("the fixture reference fits its encoding");
    let slot1 = ValueRef::Slot(1).side().expect("the fixture reference fits its encoding");

    let n1 = b.push(&eng, v4, &[ChildPair::new(pos, slot0)]).unwrap();
    let n2 = b.push(&eng, v4, &[ChildPair::new(neg, slot1)]).unwrap();
    let s0 = b.push(&eng, v5, &[ChildPair::new(pos, one)]).unwrap();
    let root_node = b.push(&eng, root_idx, &[
        ChildPair::new(n1, s0),
        ChildPair::new(n2, s0),
    ]).unwrap();

    let tdd = b
        .finish(TddNodeId { vtree: root_idx, local: root_node })
        .expect("the fixture seats its root on the node it just built");
    (f, tdd)
}

/// Force the decision phase to false on every variable this tiny instance
/// can allocate (original variables, activation literals, node variables).
/// Soundness must hold under any phase heuristic: CaDiCaL rephases
/// periodically, toward false and random phases among others, so the
/// default true phase is not a contract.
fn force_phase_false(ctx: &mut SatPruneCtx) {
    for v in 1..=128 {
        ctx.solver.phase(-v);
    }
}

/// Certified marginal boundary (distinct primes Pos/Neg): the ⊤-erased
/// boundary functions x1 / ¬x1 are disjoint, the partition certificate
/// holds, and the AllSAT enumeration soundly exhausts both slices
/// ({n1, s0, root} and {n2, s0, root} under Φ = (x2)) — every live node
/// is witnessed and the direct-kill kills nothing. This fixture is also the
/// undercount reproducer for any encoding that leaves the slot variables
/// free: the solver can then return them false, hide n1/n2/root from
/// witnessing, and the direct-kill produces DeadRoot on a satisfiable
/// instance. The ⊤-substitution removes that freedom by construction.
///
/// With the decision phase forced false, no live node is killed: no AllSAT
/// direct kill, and the root (satisfiable under Φ) is not reported dead.
#[test]
fn allsat_enumeration_sound_on_certified_marg_boundary() {
    let eng = &Engine::new();
    let (f, tdd) = marg_boundary_fixture();
    let mut ctx = sat_prune_ctx(&f);
    force_phase_false(&mut ctx);
    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());
    assert_eq!(ctx.stats.allsat_kills, 0, "an AllSAT direct kill removed a live node");
    assert!(!matches!(outcome, PruneOutcome::DeadRoot), "the root is live, got DeadRoot");
}

/// The spend cap stops pruning on unproductive spend: with a 1 ms cap and
/// 10 ms of earlier probe spend, the pass is declined and pruning stops for
/// the compile.
#[test]
fn the_spend_cap_stops_unproductive_pruning() {
    let eng = &Engine::new();
    let (f, tdd, _) = inline_marg_fixture();
    let mut ctx = sat_prune_ctx(&f);
    ctx.limits.spend_cap_ms = 1;
    ctx.stats.probe_us = 10_000;
    let size = tdd.pair_count();
    assert!(matches!(ctx.probe_tdd(eng, &tdd, size), PruneOutcome::NoOp));
    assert_eq!(ctx.stats.disabled_by, Some(PruneStop::TimeShare));
}


/// The spend cap counts only unproductive spend: spend recorded in
/// `spend_exempt_us` does not count, so the same 10 ms that stops pruning
/// above passes here, and a rewrite that removes `reset_drop_pct` percent
/// of the diagram moves the exemption up.
#[test]
fn the_spend_cap_exempts_spend_before_a_productive_rewrite() {
    let eng = &Engine::new();
    let (f, tdd, _) = inline_marg_fixture();
    let mut ctx = sat_prune_ctx(&f);
    ctx.limits.spend_cap_ms = 1;
    ctx.stats.probe_us = 10_000;
    ctx.spend_exempt_us = 10_000;
    // The fixture's rewrite drops 1 pair and keeps 3, a quarter of the
    // diagram, which 20 percent counts as productive.
    ctx.limits.reset_drop_pct = 20;
    let size = tdd.pair_count();
    assert!(
        matches!(ctx.probe_tdd(eng, &tdd, size), PruneOutcome::Rewritten(_)),
        "exempted spend must not reach the cap"
    );
    assert!(
        ctx.spend_exempt_us > 10_000,
        "a productive rewrite must move the exemption up, got {}",
        ctx.spend_exempt_us
    );
}
