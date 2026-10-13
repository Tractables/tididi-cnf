//! Tests for the sat-prune pass.

use tididi::Engine;
use tididi::diagram::{ChildRef, ChildDecoder};
use tididi::Tdd;
use super::*;
use super::ctx::{MIN_PROBES_FOR_RATE_CHECK, MIN_RUNNING_KILL_RATE};
use tididi::vtree::{Vtree, VtreeIdx};
use vitri::cnf::{Clause, CnfFormula};
use crate::test_support::{clause, formula, with_clause};

/// The diagram of one formula clause.
fn clause_tdd(vtree: &std::sync::Arc<Vtree>, c: &Clause) -> Tdd {
    Tdd::clause(vtree, c.literals.iter().copied().map(crate::convert::diagram_literal)).unwrap()
}

fn test_ctx(formula: &CnfFormula) -> SatPruneCtx {
    SatPruneCtx::new(formula, None, false).expect("CaDiCaL must be available in SAT-prune tests")
}

#[test]
fn ctx_builds_empty_formula() {
    let f = formula(0, vec![]);
    let _ctx = test_ctx(&f);
    // No solve performed at construction.
}

#[test]
fn probe_zero_tdd_returns_dead_root() {
    let eng = &Engine::new();
    // Build a trivial CNF + a constant-zero TDD; probe should immediately
    // return DeadRoot via the is_zero fast-path.
    let f = formula(2, vec![clause(&[1, 2])]);
    let mut ctx = test_ctx(&f);
    let vtree = std::sync::Arc::new(Vtree::balanced(2));
    let zero_tdd = Tdd::zero(&vtree);
    assert!(matches!(
        ctx.probe_tdd(eng, &zero_tdd, 0),
        PruneOutcome::DeadRoot
    ));
    assert_eq!(ctx.stats.passes_dead_root, 1);
}

/// Shared fixture for the inline-native + adaptive-wall tests: a
/// balanced(4) TDD whose v4 level holds p = [(Pos, Inline(5))] (live)
/// and p2 = [(Neg, Inline(0))] (dead-by-count) over a marginalized
/// leaf child with zero overflow slots. The formula x1 ∧ x3 implies the
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
    // Marginalize the RIGHT leaf-child of v4; all its refs are inline,
    // so the level has zero overflow slots (3a must skip it — a slot-
    // shaped leaf level cannot take the implicit Pos/Neg/One writes).
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

/// Inline-native regression: sat-prune must handle marg-side INLINE refs
/// (bit-30 set, count in the low 30 bits) end-to-end — encode resolves
/// them statically (count 0 ⇒ dead contribution, count > 0 ⇒ constant-⊤),
/// probe kills the dead parent, and the rewrite passes surviving inline
/// refs through bit-30-intact. A `probe_tdd` that declined every pass on
/// seeing inline marg refs fails this test on the Rewritten match.
#[test]
fn inline_marg_refs_probe_and_rewrite_natively() {
    let eng = &Engine::new();
    use tididi::diagram::ValueRef;

    let (f, tdd, v4) = inline_marg_fixture();
    let mut ctx = test_ctx(&f);
    let size = tdd.pair_count();
    match ctx.probe_tdd(eng, &tdd, size) {
        PruneOutcome::Rewritten(new_tdd) => {
            assert!(
                ctx.stats.nodes_pruned >= 1,
                "p2 (inline count 0) must be pruned, nodes_pruned = {}",
                ctx.stats.nodes_pruned
            );
            // p survives alone at v4 with its inline ref bit-30-intact.
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

/// Force the decision phase FALSE on every var this tiny instance can
/// allocate (orig vars, activation lits, y vars). Soundness must hold
/// for ANY phase heuristic — CaDiCaL rephases periodically, including
/// toward false/random, so the default-true phase is not a contract.
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
/// free: the solver can then return them FALSE, hide n1/n2/root from
/// witnessing, and the direct-kill produces DeadRoot on a satisfiable
/// instance. The ⊤-substitution removes that freedom by construction.
///
/// With the decision phase forced false, no live node is killed: no AllSAT
/// direct kill, and the root (satisfiable under Φ) is not reported dead.
#[test]
fn allsat_enumeration_sound_on_certified_marg_boundary() {
    let eng = &Engine::new();
    let (f, tdd) = marg_boundary_fixture();
    let mut ctx = test_ctx(&f);
    force_phase_false(&mut ctx);
    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());
    assert_eq!(ctx.stats.allsat_kills, 0, "an AllSAT direct kill removed a live node");
    assert!(!matches!(outcome, PruneOutcome::DeadRoot), "the root is live, got DeadRoot");
}

/// Adaptive wall trips on unproductive spend: with a 1 ms ceiling and
/// 10 ms of pre-existing probe spend, the pass is declined and sat-prune
/// sticky-disables.
#[test]
fn wall_trips_on_unproductive_spend() {
    let eng = &Engine::new();
    let (f, tdd, _) = inline_marg_fixture();
    let mut ctx = test_ctx(&f);
    ctx.wall_ms = 1;
    ctx.stats.probe_us = 10_000;
    let size = tdd.pair_count();
    assert!(matches!(ctx.probe_tdd(eng, &tdd, size), PruneOutcome::NoOp));
    assert!(
        ctx.disabled_by == Some("time-share"),
        "wall must attribute the disable"
    );
}

/// Productivity-aware wall: spend snapshotted in `wall_exempt_us` does
/// not count toward the ceiling — the same 10 ms that trips the wall
/// above passes here, and a rewrite clearing `wall_reset_drop_pct`
/// advances the snapshot.
#[test]
fn wall_exempts_spend_before_productive_rewrite() {
    let eng = &Engine::new();
    let (f, tdd, _) = inline_marg_fixture();
    let mut ctx = test_ctx(&f);
    ctx.wall_ms = 1;
    ctx.stats.probe_us = 10_000;
    ctx.wall_exempt_us = 10_000;
    // Fixture drop fraction is 1 dropped / (1 + 3 surviving) = 25%;
    // 20% threshold makes this rewrite "productive".
    ctx.wall_reset_drop_pct = 20;
    let size = tdd.pair_count();
    assert!(
        matches!(ctx.probe_tdd(eng, &tdd, size), PruneOutcome::Rewritten(_)),
        "exempted spend must not trip the wall"
    );
    assert!(
        ctx.wall_exempt_us > 10_000,
        "productive rewrite must advance the meter snapshot, got {}",
        ctx.wall_exempt_us
    );
}

fn prefix_fixture() -> (std::sync::Arc<Vtree>, Tdd, CnfFormula) {
    // S = (x1∨x3) ∧ (x2∨¬x3) on the left-linear vtree, F = S ∧ (¬x1).
    // Two of the three nodes at the bottom internal level require x1=1
    // and are dead under F.
    let vtree = std::sync::Arc::new(Vtree::reverse_linear(3));
    let s = [clause(&[1, 3]), clause(&[2, -3])];
    let tdd = clause_tdd(&vtree, &s[0]) & clause_tdd(&vtree, &s[1]);
    let f = formula(3, [s.to_vec(), vec![clause(&[-1])]].concat());
    (vtree, tdd, f)
}

/// With the encode cut after the first internal level, the bottom level is
/// fully defined and its two dead nodes are still killed, while the unencoded
/// root level is skipped rather than aborting the pass.
#[test]
fn an_encode_prefix_is_probed() {
    let eng = &Engine::new();
    let (vtree, tdd, f) = prefix_fixture();
    let mut ctx = test_ctx(&f);
    ctx.max_encode_levels = 1;
    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());
    assert_eq!(ctx.stats.passes_encode_prefix, 1);
    assert_eq!(ctx.stats.passes_aborted_encode_budget, 0);
    assert!(ctx.stats.prefix_nodes_skipped >= 1, "root level skipped");
    let PruneOutcome::Rewritten(new_tdd) = outcome else {
        panic!("expected Rewritten, got {outcome:?}");
    };
    assert!(ctx.stats.nodes_pruned >= 2, "both dead bottom nodes are killed");
    let unit = clause_tdd(&vtree, &clause(&[-1]));
    let conj = new_tdd & unit;
    assert_eq!(conj.model_count().unwrap(), num_bigint::BigUint::from(1u32));
}

/// The per-pass wall-clock terminator bounds each `solve()` call, not just
/// the checks between probes. PHP(8,7) takes real CDCL work to refute (tens
/// of milliseconds); with `wall_ms = 1` and `solve_overrun_ms = 0` the
/// terminator's deadline is 1 ms into the pass, so every probe comes back
/// UNKNOWN. A solve left to run refutes PHP and counts as UNSAT.
#[test]
fn pass_deadline_bounds_single_solve() {
    let eng = &Engine::new();

    // PHP(8,7): 8 pigeons, 7 holes — 56 variables.
    // UNSAT; refutation requires substantial CDCL work (well over 1ms).
    let n_pigeons: u32 = 8;
    let n_holes: u32 = 7;
    let num_vars = n_pigeons * n_holes; // 56

    let p = |pigeon: u32, hole: u32| (pigeon * n_holes + hole + 1) as i32;

    let mut clauses = Vec::new();
    // Each pigeon must be in some hole.
    for i in 0..n_pigeons {
        clauses.push(clause(&(0..n_holes).map(|j| p(i, j)).collect::<Vec<_>>()));
    }
    // No two pigeons share a hole.
    for j in 0..n_holes {
        for i1 in 0..n_pigeons {
            for i2 in (i1 + 1)..n_pigeons {
                clauses.push(clause(&[-p(i1, j), -p(i2, j)]));
            }
        }
    }

    let f = formula(num_vars, clauses);

    // TDD: a single clause over two PHP vars (p_{0,0} ∨ p_{0,1}).
    // Its nodes are reachable internals to probe; PHP is UNSAT overall,
    // so without the terminator every probe returns UNSAT.
    let vtree = std::sync::Arc::new(Vtree::balanced(num_vars));
    let tdd = clause_tdd(&vtree, &clause(&[p(0, 0), p(0, 1)]));

    let mut ctx = test_ctx(&f);

    // 1ms pass budget. The AllSAT prelude is off so the per-node probes
    // run, and the conflict budget is high enough that the conflict limit
    // cannot stop a solve before the terminator does.
    ctx.wall_ms = 1;
    ctx.solve_overrun_ms = 0;
    ctx.allsat_k = 0;
    ctx.probe_conflict_budget = 100_000;

    let size = tdd.pair_count();
    ctx.probe_tdd(eng, &tdd, size);

    assert_eq!(
        ctx.stats.probes_unsat, 0,
        "terminator must abort solve() before PHP refutation completes; \
             got probes_unsat={} (probes_unknown={})",
        ctx.stats.probes_unsat, ctx.stats.probes_unknown,
    );
}

#[test]
fn encode_prefix_zero_levels_falls_back_to_abort() {
    let eng = &Engine::new();
    // Nothing encoded ⇒ prefix mode is not eligible; the pass aborts
    // (retires its activation, NoOp).
    let (_vtree, tdd, f) = prefix_fixture();
    let mut ctx = test_ctx(&f);
    ctx.max_encode_levels = 0;
    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());
    assert!(matches!(outcome, PruneOutcome::NoOp), "got {outcome:?}");
    assert_eq!(ctx.stats.passes_encode_prefix, 0);
    assert_eq!(ctx.stats.passes_aborted_encode_budget, 1);
}

// ── Running-rate gate: when it is evaluated ─────────────────────────
//
// The gate runs every `MIN_PROBES_FOR_RATE_CHECK` probes inside the walk,
// so a hopeless walk exits at its first check point and a walk that is
// killing keeps going.

/// A wide TDD over a linear vtree: `n_vars` variables tied together by
/// long clauses `(x_i ∨ x_{i+span})`, which force the compiled diagram to
/// carry state across the middle of the variable order. Returns the TDD
/// and the CNF of exactly those clauses, so probing the TDD against its
/// own formula cannot kill anything (every reachable node lies on some
/// model of Φ).
fn wide_alive_fixture(n_vars: u32, span: u32) -> (Tdd, CnfFormula) {
    let vtree = std::sync::Arc::new(Vtree::linear(n_vars));
    let clauses: Vec<Clause> = (1..=(n_vars - span) as i32)
        .map(|v| clause(&[v, v + span as i32]))
        .collect();
    let tdd = clauses
        .iter()
        .map(|c| clause_tdd(&vtree, c))
        .reduce(|a, b| a & b)
        .expect("fixture has at least one clause");
    let f = formula(n_vars, clauses);
    (tdd, f)
}

/// Count reachable internal nodes — the upper bound on how many probes a
/// full walk would issue. Used to keep the schedule assertions honest: if
/// the fixture offered fewer nodes than the check cadence, "stopped at the
/// first check point" would hold for the wrong reason.
fn probeable_nodes(tdd: &Tdd) -> usize {
    let reachable = tdd.reachable_nodes();
    tdd.vtree()
        .internal_bottomup()
        .map(|(t, _, _)| {
            let nodes = tdd.levels()[t.idx()].nodes();
            (0..nodes.len()).filter(|&i| reachable[t.idx()][i]).count()
        })
        .sum()
}

/// Zero-kill walk: every probe returns SAT, so the cumulative kill rate is
/// 0 at every prefix and the gate's verdict is already reachable at the
/// first check point. The walk must stop there.
#[test]
fn running_rate_disables_at_first_check_point_on_zero_kill_walk() {
    let eng = &Engine::new();
    let (tdd, f) = wide_alive_fixture(14, 7);
    let mut ctx = test_ctx(&f);
    // Force one real probe per node: the AllSAT prelude would otherwise
    // witness most nodes alive and skip their probes.
    ctx.allsat_k = 0;

    let cadence = MIN_PROBES_FOR_RATE_CHECK as usize;
    assert!(
        probeable_nodes(&tdd) > cadence,
        "fixture must offer more probes ({}) than the check cadence ({cadence}), \
             else the stop-at-cadence assertion is vacuous",
        probeable_nodes(&tdd),
    );

    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());

    assert_eq!(
        ctx.stats.probes_unsat, 0,
        "fixture probes its own formula: nothing can be dead"
    );
    assert_eq!(
        ctx.stats.nodes_pruned, 0,
        "fixture probes its own formula: nothing can be dead"
    );
    assert_eq!(
        ctx.stats.probes_attempted, MIN_PROBES_FOR_RATE_CHECK,
        "walk must stop at the first running-rate check, not run to the end \
             of the level walk"
    );
    assert!(
        ctx.disabled_by.is_some(),
        "zero kill rate at the check point must sticky-disable"
    );
    assert_eq!(ctx.stats_snapshot().disabled_by, "low-kill-rate");
    assert!(
        matches!(outcome, PruneOutcome::NoOp),
        "a kill-free pass returns NoOp; got {outcome:?}"
    );
}

/// A walk whose kill rate clears the threshold at the check point is not
/// cut short. Same fixture shape, but Φ adds
/// units that make part of the diagram unreachable, so probes kill.
#[test]
fn running_rate_keeps_probing_when_kill_rate_clears_threshold() {
    let eng = &Engine::new();
    let (tdd, mut f) = wide_alive_fixture(20, 10);
    // Force most of the low half false: every node that survives only via
    // one of these dies, so probes keep landing UNSAT well past the first
    // check point, while the diagram stays large enough to keep probing.
    for v in 1..=8 {
        f = with_clause(f, clause(&[-v]));
    }
    let mut ctx = test_ctx(&f);
    ctx.allsat_k = 0;

    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());

    let attempted = ctx.stats.probes_attempted;
    let unsat = ctx.stats.probes_unsat;
    assert!(
        attempted > MIN_PROBES_FOR_RATE_CHECK,
        "walk must continue past the first check point when it is killing \
             (attempted={attempted}, unsat={unsat})"
    );
    assert!(
        (unsat as f32 / attempted as f32) >= MIN_RUNNING_KILL_RATE,
        "fixture must sustain a kill rate above the threshold \
             (unsat={unsat}/{attempted})"
    );
    assert!(
        ctx.disabled_by.is_none(),
        "a productive walk must not be disabled (unsat={unsat}/{attempted})"
    );
    assert!(
        matches!(outcome, PruneOutcome::Rewritten(_) | PruneOutcome::DeadRoot),
        "kills must reach the rewrite; got {outcome:?}"
    );
}
