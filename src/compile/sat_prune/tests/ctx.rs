use tididi::vtree::Vtree;
use tididi::{Engine, Tdd};
use vitri::cnf::{Clause, CnfFormula};

use crate::compile::sat_prune::{PruneOutcome, PruneStop};
use crate::test_support::{clause, formula, sat_prune_ctx, with_clause};

/// The diagram of one formula clause.
fn clause_tdd(vtree: &std::sync::Arc<Vtree>, c: &Clause) -> Tdd {
    Tdd::clause(vtree, c.literals.iter().copied().map(crate::convert::diagram_literal)).unwrap()
}

/// A formula with no variable still builds a context.
#[test]
fn a_formula_without_variables_builds_a_context() {
    sat_prune_ctx(&formula(0, vec![]));
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
    let mut ctx = sat_prune_ctx(&f);
    ctx.limits.max_encode_levels = 1;
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

/// The pass's terminator bounds each `solve()` call, not just the checks
/// between probes. PHP(8,7) takes real CDCL work to refute; with
/// `spend_cap_ms = 1` and `solve_overrun_ms = 0` the terminator's deadline
/// is 1 ms into the pass, so every probe comes back unknown. A solve left
/// to run refutes PHP and counts as UNSAT.
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

    let mut ctx = sat_prune_ctx(&f);

    // A 1 ms pass budget. The enumeration is off so the per-node probes
    // run, and the conflict budget is high enough that the conflict limit
    // cannot stop a solve before the terminator does.
    ctx.limits.spend_cap_ms = 1;
    ctx.limits.solve_overrun_ms = 0;
    ctx.limits.allsat_k = 0;
    ctx.limits.probe_conflict_budget = 100_000;

    let size = tdd.pair_count();
    ctx.probe_tdd(eng, &tdd, size);

    assert_eq!(
        ctx.stats.probes_unsat, 0,
        "terminator must abort solve() before PHP refutation completes; \
             got probes_unsat={} (probes_unknown={})",
        ctx.stats.probes_unsat, ctx.stats.probes_unknown,
    );
}

/// With the encode cut before the first level nothing is encoded, so there
/// is no prefix to probe: the pass retires its activation literal and
/// returns `NoOp`.
#[test]
fn encode_prefix_zero_levels_falls_back_to_abort() {
    let eng = &Engine::new();
    let (_vtree, tdd, f) = prefix_fixture();
    let mut ctx = sat_prune_ctx(&f);
    ctx.limits.max_encode_levels = 0;
    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());
    assert!(matches!(outcome, PruneOutcome::NoOp), "got {outcome:?}");
    assert_eq!(ctx.stats.passes_encode_prefix, 0);
    assert_eq!(ctx.stats.passes_aborted_encode_budget, 1);
}

// ── The kill-rate gate: when it is evaluated ────────────────────────
//
// The gate runs every `min_probes_for_rate_check` probes inside the walk,
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
    let mut ctx = sat_prune_ctx(&f);
    // Force one real probe per node: the enumeration would otherwise
    // witness most nodes alive and skip their probes.
    ctx.limits.allsat_k = 0;

    let cadence = ctx.limits.min_probes_for_rate_check;
    assert!(
        probeable_nodes(&tdd) > cadence as usize,
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
        ctx.stats.probes_attempted, cadence,
        "walk must stop at the first kill-rate check, not run to the end \
             of the level walk"
    );
    assert_eq!(
        ctx.stats.disabled_by,
        Some(PruneStop::LowKillRate),
        "a zero kill rate at the check point stops pruning"
    );
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
    let mut ctx = sat_prune_ctx(&f);
    ctx.limits.allsat_k = 0;

    let outcome = ctx.probe_tdd(eng, &tdd, tdd.pair_count());

    let attempted = ctx.stats.probes_attempted;
    let unsat = ctx.stats.probes_unsat;
    assert!(
        attempted > ctx.limits.min_probes_for_rate_check,
        "walk must continue past the first check point when it is killing \
             (attempted={attempted}, unsat={unsat})"
    );
    assert!(
        (unsat as f32 / attempted as f32) >= ctx.limits.min_kill_rate,
        "fixture must sustain a kill rate above the threshold \
             (unsat={unsat}/{attempted})"
    );
    assert!(
        ctx.stats.disabled_by.is_none(),
        "a productive walk must not stop pruning (unsat={unsat}/{attempted})"
    );
    assert!(
        matches!(outcome, PruneOutcome::Rewritten(_) | PruneOutcome::DeadRoot),
        "kills must reach the rewrite; got {outcome:?}"
    );
}
