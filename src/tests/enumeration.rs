use std::time::Duration;

use num_bigint::BigUint;
use vitri::cnf::{Clause, CnfFormula};

use tididi::test_helpers::rat;
use tididi::vtree::VarId;

use super::{
    AllsatStop, AllsatCtl, Models, WallGrant, WeightedModels, allsat, constrained_show_count,
    estimate_count_by_random_paths, forecast_exceeds_wall, path_seed, split_show, sweep,
};
use crate::test_support::lit;

/// Show vars 0 and 2 occur (2 three times) and are constrained; show var 3
/// does not occur and is free; var 1 is hidden.
#[test]
fn counts_each_occurring_show_var_once() {
    let f = crate::test_support::formula(4, vec![
        Clause::new(vec![lit(2, true), lit(0, true)]),
        Clause::new(vec![lit(2, false), lit(1, true)]),
        Clause::new(vec![lit(2, true)]),
    ]);
    assert_eq!(constrained_show_count(&f, &crate::problem::ShowSet::from_dimacs_ids(&[1, 3, 4]).unwrap()), 2);
}

/// Pigeonhole `PHP(n, n-1)`: n pigeons into n−1 holes. UNSAT, and famously
/// hard for resolution — so the first `solve()` alone runs for a long time, which
/// is exactly the shape the wall enforcement has to bound.
fn pigeonhole(pigeons: u32) -> CnfFormula {
    let holes = pigeons - 1;
    let v = |p: u32, h: u32| p * holes + h;
    let mut clauses = Vec::new();
    for p in 0..pigeons {
        clauses.push(Clause::new(
            (0..holes).map(|h| lit(v(p, h), true)).collect(),
        ));
    }
    for h in 0..holes {
        for p1 in 0..pigeons {
            for p2 in (p1 + 1)..pigeons {
                clauses.push(Clause::new(vec![
                    lit(v(p1, h), false),
                    lit(v(p2, h), false),
                ]));
            }
        }
    }
    crate::test_support::formula(pigeons * holes, clauses)
}

/// A show set whose feasible projections are a subcube: `forced` vars pinned
/// by unit clauses, `free` vars occurring but unconstrained. Feasible
/// projections = 2^free, and every random path must weigh exactly that.
fn subcube_formula(forced: u32, free: u32) -> CnfFormula {
    let mut clauses = Vec::new();
    for v in 0..forced {
        clauses.push(Clause::new(vec![lit(v, true)]));
    }
    // Each free var occurs (so it counts as constrained in the split sense)
    // in a clause already satisfied by a forced var, leaving it a free choice.
    for v in 0..free {
        clauses.push(Clause::new(vec![lit(0, true), lit(forced + v, true)]));
        clauses.push(Clause::new(vec![lit(0, true), lit(forced + v, false)]));
    }
    crate::test_support::formula(forced + free, clauses)
}

/// The count estimate does not collapse to zero when most show vars are
/// (near-)forced. A random-restriction estimator fixes show vars to
/// random polarities, which contradicts a forced unit almost surely, and so
/// reports N̂=0 on a formula whose feasible set is large — on which a caller
/// reading zero as "small" commits to an enumeration that will not finish. A
/// random path cannot report 0: a forced var contributes a factor of 1, a free
/// one a factor of 2.
#[test]
fn random_paths_are_exact_on_a_subcube_of_mostly_forced_vars() {
    let f = subcube_formula(18, 2);
    let show: Vec<VarId> = (1..=f.num_vars()).map(VarId).collect();
    let est = estimate_count_by_random_paths(&f, &show, 6, Duration::from_secs(5), 7);
    assert!(!est.unsat);
    assert_eq!(est.truncated, 0);
    assert_eq!(est.weights.len(), 6);
    // 2 free vars ⇒ 4 feasible projections, on every path.
    assert!(
        est.weights.iter().all(|&w| w == 4.0),
        "weights {:?}",
        est.weights
    );
    assert_eq!(est.mean(), Some(4.0));
    // And that is the count the enumerator itself reports.
    let exact = allsat(
        &f,
        &split_show(&f, &show),
        1_000_000,
        Duration::from_secs(10),
        &mut AllsatCtl::default(),
        Models(0),
    );
    assert_eq!(exact, Some(BigUint::from(4u32)));
}

/// The estimator is unbiased on a non-product feasible set too: for
/// `(v0) ∧ (v1 ∨ v2)` over show {v0,v1,v2} the exact count is 3 and the two
/// possible path weights (2 and 4) average to it.
#[test]
fn random_path_weights_bracket_a_non_product_count() {
    let f = crate::test_support::formula(3, vec![
        Clause::new(vec![lit(0, true)]),
        Clause::new(vec![lit(1, true), lit(2, true)]),
    ]);
    let show = [VarId(1), VarId(2), VarId(3)];
    let est = estimate_count_by_random_paths(&f, &show, 16, Duration::from_secs(5), 11);
    assert_eq!(est.weights.len(), 16);
    assert!(
        est.weights.iter().all(|&w| w == 2.0 || w == 4.0),
        "{:?}",
        est.weights
    );
    let mean = est.mean().unwrap();
    assert!((2.0..=4.0).contains(&mean), "mean {mean}");
    assert_eq!(
        allsat(
            &f,
            &split_show(&f, &show),
            1_000_000,
            Duration::from_secs(10),
            &mut AllsatCtl::default(),
            Models(0),
        ),
        Some(BigUint::from(3u32)),
    );
}

/// A path whose solves outrun the path wall is reported as truncated, not as
/// a small weight — `mean()` must stay `None` so a caller cannot mistake "not
/// measured" for "small".
#[test]
fn a_cut_path_reports_truncated_and_no_mean() {
    let f = pigeonhole(11);
    let show: Vec<VarId> = (1..=f.num_vars()).map(VarId).collect();
    let est = estimate_count_by_random_paths(&f, &show, 3, Duration::from_millis(20), 3);
    assert!(est.weights.is_empty());
    assert_eq!(est.mean(), None);
    assert_eq!(est.truncated, 3);
    assert!(!est.unsat);
}

/// Under a wall callback the wall budget bounds even a single solve, not just
/// the gap between solves. Without the connected terminator this call
/// would sit inside one CaDiCaL solve for minutes, whatever budget it was
/// given.
#[test]
fn a_supervised_wall_cuts_a_single_long_solve() {
    let f = pigeonhole(11);
    let show: Vec<VarId> = (1..=f.num_vars()).map(VarId).collect();
    let mut decline = |_models: u64, _elapsed: Duration| -> Option<WallGrant> { None };
    let mut ctl = AllsatCtl {
        at_wall: Some(&mut decline),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let got = allsat(&f, &split_show(&f, &show), 1_000_000, Duration::from_millis(50), &mut ctl, Models(0));
    let elapsed = t.elapsed();
    assert_eq!(got, None);
    assert_eq!(ctl.bust, Some(AllsatStop::Terminated));
    assert!(
        elapsed < Duration::from_secs(10),
        "a 50ms budget must not run a solve for {elapsed:?}",
    );
}

/// The wall callback resumes the same enumeration: a first budget too small
/// to finish, extended once under a bound the true count stays under, must
/// reach the identical exact count — the blocking-clause DB and model tally
/// carry over rather than restarting, and a bound the count respects
/// changes nothing.
#[test]
fn at_wall_resumes_to_the_exact_count() {
    let f = two_show_formula();
    let show = [VarId(1), VarId(2)];
    let calls = std::cell::Cell::new(0u32);
    let mut resume = |_models: u64, _elapsed: Duration| -> Option<WallGrant> {
        calls.set(calls.get() + 1);
        Some(WallGrant {
            extra: Duration::from_secs(5),
            // A bound the true count (3) stays under.
            abort_at_models: 5,
        })
    };
    let mut ctl = AllsatCtl {
        at_wall: Some(&mut resume),
        ..Default::default()
    };
    // Zero budget ⇒ the callback fires on the very first wall check.
    let got = allsat(&f, &split_show(&f, &show), 1_000, Duration::from_millis(0), &mut ctl, Models(0));
    assert_eq!(got, Some(BigUint::from(3u32)));
    assert_eq!(ctl.models, 3);
    assert_eq!(ctl.bust, None);
    assert_eq!(calls.get(), 1, "one extension is enough to finish");
}

/// A callback that declines busts, and the reported reason is the wall (not
/// the model cap).
#[test]
fn at_wall_decline_busts_with_the_wall_reason() {
    let f = two_show_formula();
    let show = [VarId(1), VarId(2)];
    let mut decline = |_models: u64, _elapsed: Duration| -> Option<WallGrant> { None };
    let mut ctl = AllsatCtl {
        at_wall: Some(&mut decline),
        ..Default::default()
    };
    let got = allsat(&f, &split_show(&f, &show), 1_000, Duration::from_millis(0), &mut ctl, Models(0));
    assert_eq!(got, None);
    assert_eq!(ctl.bust, Some(AllsatStop::Wall));
}

/// A bounded grant is abandoned the moment the enumeration passes its bound:
/// `two_show_formula` has 3 feasible projections, so a grant bounded at 2 must
/// stop at 3 with the ceiling reason and the partial tally, rather than run
/// to completion or report the model cap.
#[test]
fn a_tally_past_the_ceiling_aborts_the_granted_window() {
    let f = two_show_formula();
    let show = [VarId(1), VarId(2)];
    let mut resume = |_models: u64, _elapsed: Duration| -> Option<WallGrant> {
        Some(WallGrant {
            extra: Duration::from_secs(5),
            // A ceiling the true count (3) exceeds.
            abort_at_models: 2,
        })
    };
    let mut ctl = AllsatCtl {
        at_wall: Some(&mut resume),
        ..Default::default()
    };
    // Zero opening budget ⇒ the grant is in force from the first model, and
    // the model cap (1000) is far away, so only the ceiling can trip.
    let got = allsat(&f, &split_show(&f, &show), 1_000, Duration::from_millis(0), &mut ctl, Models(0));
    assert_eq!(got, None);
    assert_eq!(ctl.bust, Some(AllsatStop::TallyCeiling));
    assert_eq!(
        ctl.models, 3,
        "stops one past the bound, tally is a lower bound"
    );
}

/// The model cap reports its own reason, and the partial tally is a lower
/// bound the caller can extrapolate from. The cap check reads only the
/// feasible-projection count, before any tally is folded in, so it falls
/// through the same way under a weighted tally.
#[test]
fn model_cap_bust_reports_the_cap_and_a_partial_tally() {
    let f = two_show_formula();
    let show = [VarId(1), VarId(2)];
    let mut decline = |_models: u64, _elapsed: Duration| -> Option<WallGrant> { None };
    let mut ctl = AllsatCtl {
        at_wall: Some(&mut decline),
        ..Default::default()
    };
    let got = allsat(&f, &split_show(&f, &show), 1, Duration::from_secs(5), &mut ctl, Models(0));
    assert_eq!(got, None);
    assert_eq!(ctl.bust, Some(AllsatStop::ModelCap));
    assert_eq!(ctl.models, 2, "one past the cap of 1");

    let weights = vec![(rat(1, 1), rat(1, 1)); 2];
    let weighted = allsat(
        &f, &split_show(&f, &show), 1, Duration::from_secs(5), &mut AllsatCtl::default(), WeightedModels::new(&weights),
    );
    assert!(weighted.is_none(), "the cap busts a weighted tally too");
}

/// The count forecast stops an enumeration heading for far more projections
/// than its wall holds: one forced and 60 free constrained vars make 2^60
/// feasible projections, which every random path weighs exactly, against a
/// 30 s wall. A forecast at zero is taken after the first model, and the stop
/// reports the estimate and the tally.
#[test]
fn a_forecast_stops_a_count_the_wall_cannot_hold() {
    let f = subcube_formula(1, 60);
    let show: Vec<VarId> = (1..=f.num_vars()).map(VarId).collect();
    let mut ctl = AllsatCtl { forecast: Some(Duration::ZERO), ..Default::default() };
    let started = std::time::Instant::now();
    let got = allsat(&f, &split_show(&f, &show), u64::MAX, Duration::from_secs(30), &mut ctl, Models(0));
    assert_eq!(got, None);
    assert_eq!(ctl.bust, Some(AllsatStop::Forecast(2f64.powi(60))));
    assert_eq!(ctl.models, 1, "the forecast is taken after the first model");
    assert!(started.elapsed() < Duration::from_secs(10), "stopped at once, not at the wall");
}

/// A count the wall holds is enumerated to the end under a forecast: four
/// feasible projections, forecast after the first model.
#[test]
fn a_forecast_lets_a_small_count_finish() {
    let f = subcube_formula(18, 2);
    let show: Vec<VarId> = (1..=f.num_vars()).map(VarId).collect();
    let mut ctl = AllsatCtl { forecast: Some(Duration::ZERO), ..Default::default() };
    let got = allsat(&f, &split_show(&f, &show), 1_000_000, Duration::from_secs(30), &mut ctl, Models(0));
    assert_eq!(got, Some(BigUint::from(4u32)));
    assert_eq!(ctl.bust, None);
}

/// The forecast rule on its own: the projections still missing, at the rate
/// so far, against twice the wall left. An estimate at or under the tally
/// keeps enumerating, and so does a spent wall, which the wall check reports.
#[test]
fn the_forecast_rule_compares_the_missing_projections_with_twice_the_wall_left() {
    let s = Duration::from_secs;
    // 1000 projections in 5 s is 200/s, with 55 s left; 1e6 needs about
    // 5000 s.
    assert!(forecast_exceeds_wall(1e6, 1000, s(5), s(55)));
    // 20 000 needs 95 s, inside twice the 55 s left.
    assert!(!forecast_exceeds_wall(20_000.0, 1000, s(5), s(55)));
    // 23 001 needs just over 110 s.
    assert!(forecast_exceeds_wall(23_001.0, 1000, s(5), s(55)));
    assert!(!forecast_exceeds_wall(500.0, 1000, s(5), s(55)));
    assert!(!forecast_exceeds_wall(1e9, 1000, s(5), Duration::ZERO));
    assert!(!forecast_exceeds_wall(1e9, 0, s(5), s(55)));
}

/// The engine's generator starts at `seed | 1`: the paths' seeds must stay
/// distinct through it, or two paths walk the same way.
#[test]
fn path_seeds_give_distinct_generator_states() {
    let states: std::collections::HashSet<u64> = (0..64).map(|k| path_seed(k) | 1).collect();
    assert_eq!(states.len(), 64);
    // Path 0 keeps the base seed.
    assert_eq!(path_seed(0), 0x9E37_79B9_7F4A_7C15);
}

/// Formula (x0 ∨ x1) over two show vars; feasible projections are
/// {TT, TF, FT} (FF excluded), so PMC = 3, and with weights a and b on x0 and
/// ¬x0, c and d on x1 and ¬x1, PWMC = ac+ad+bc.
fn two_show_formula() -> CnfFormula {
    crate::test_support::formula(2, vec![Clause::new(vec![lit(0, true), lit(1, true)])])
}

/// The weighted-enumeration cases — feasible-projection weights, unit
/// weights equal to the unweighted PMC count, a free show var's
/// (w_pos+w_neg) factor, satisfiable formulas that weigh zero, and an
/// unsatisfiable one — checked through both enumerators: the 2^K sweep
/// and the blocking-clause AllSAT walk. Both must reach the same total
/// and the same verdict.
#[test]
fn pwmc_matches_across_the_sweep_and_the_allsat_walk() {
    use crate::problem::WeightedCount::{Satisfiable, Unsatisfiable};
    let free_show_formula = || {
        crate::test_support::formula(3, vec![Clause::new(vec![lit(0, true), lit(1, true)])])
    };
    let contradiction = || {
        crate::test_support::formula(2, vec![
            Clause::new(vec![lit(0, true), lit(1, true)]),
            Clause::new(vec![lit(0, false)]),
            Clause::new(vec![lit(1, false)]),
        ])
    };
    // (label, formula, show, (w_neg, w_pos) per variable, expected total)
    let cases = [
        (
            "feasible-projection weights: ac+ad+bc = 10+14+15",
            two_show_formula(),
            vec![VarId(1), VarId(2)],
            vec![(rat(3, 1), rat(2, 1)), (rat(7, 1), rat(5, 1))],
            Satisfiable(rat(39, 1)),
        ),
        (
            "unit weights equal the unweighted PMC count",
            two_show_formula(),
            vec![VarId(1), VarId(2)],
            vec![(rat(1, 1), rat(1, 1)), (rat(1, 1), rat(1, 1))],
            Satisfiable(rat(3, 1)),
        ),
        (
            // x2 (VarId 2) is a show var absent from all clauses → free show,
            // factor (w_pos+w_neg)=2 on top of the two-clause core PWMC=39.
            "a free show var's factor",
            free_show_formula(),
            vec![VarId(1), VarId(2), VarId(3)],
            vec![(rat(3, 1), rat(2, 1)), (rat(7, 1), rat(5, 1)), (rat(1, 1), rat(1, 1))],
            Satisfiable(rat(78, 1)),
        ),
        (
            "x0 weighed zero both ways: every projection weighs zero",
            two_show_formula(),
            vec![VarId(1), VarId(2)],
            vec![(rat(0, 1), rat(0, 1)), (rat(1, 1), rat(1, 1))],
            Satisfiable(rat(0, 1)),
        ),
        (
            "a free show var whose signed weights cancel",
            free_show_formula(),
            vec![VarId(1), VarId(2), VarId(3)],
            vec![(rat(1, 1), rat(1, 1)), (rat(1, 1), rat(1, 1)), (rat(-1, 1), rat(1, 1))],
            Satisfiable(rat(0, 1)),
        ),
        (
            "no projection is feasible",
            contradiction(),
            vec![VarId(1), VarId(2)],
            vec![(rat(1, 1), rat(1, 1)), (rat(1, 1), rat(1, 1))],
            Unsatisfiable,
        ),
        (
            "nothing constrained is shown, and the formula has no model",
            contradiction(),
            vec![],
            vec![(rat(1, 1), rat(1, 1)), (rat(1, 1), rat(1, 1))],
            Unsatisfiable,
        ),
    ];
    for (label, f, show, weights, expected) in cases {
        let swept = sweep(&f, &split_show(&f, &show), WeightedModels::new(&weights), None).unwrap();
        assert_eq!(swept, expected, "{label}: sweep");
        let walked = allsat(
            &f, &split_show(&f, &show), 1000, Duration::from_secs(5), &mut AllsatCtl::default(), WeightedModels::new(&weights),
        )
        .unwrap();
        assert_eq!(walked, expected, "{label}: allsat");
    }
}

/// The independent oracle, over the show vars' zero-based indices.
fn brute_projected_count(formula: &CnfFormula, show_vars: &[VarId]) -> BigUint {
    crate::test_support::brute_projected_count(formula, show_vars.iter().map(|v| v.idx()))
}

/// The shared battery of micro-formulas every enumerator is checked against:
/// full and partial feasibility, projected (hidden) vars, free show vars, a
/// projection-UNSAT case, and an empty constrained set.
fn micro_cases() -> Vec<(CnfFormula, Vec<VarId>)> {
    vec![
        // (x0∨x1): feasible {TT,TF,FT} → 3.
        (two_show_formula(), vec![VarId(1), VarId(2)]),
        // (x0∨x1)∧(¬x0∨¬x1): feasible {TF,FT} → 2.
        (
            crate::test_support::formula(2, vec![
                Clause::new(vec![lit(0, true), lit(1, true)]),
                Clause::new(vec![lit(0, false), lit(1, false)]),
            ]),
            vec![VarId(1), VarId(2)],
        ),
        // free show var x2 absent from clauses → factor 2 (3<<1=6).
        (
            crate::test_support::formula(3, vec![Clause::new(vec![lit(0, true), lit(1, true)])]),
            vec![VarId(1), VarId(2), VarId(3)],
        ),
        // projected (hidden) var p=x2: clauses (x0∨p)∧(x1∨¬p), show={x0,x1}.
        // Feasible projections: TT,TF,FT (FF needs p=T and p=F) → 3.
        (
            crate::test_support::formula(3, vec![
                Clause::new(vec![lit(0, true), lit(2, true)]),
                Clause::new(vec![lit(1, true), lit(2, false)]),
            ]),
            vec![VarId(1), VarId(2)],
        ),
        // projection-UNSAT: (x0)∧(¬x0) → 0 feasible.
        (
            crate::test_support::formula(1, vec![
                Clause::new(vec![lit(0, true)]),
                Clause::new(vec![lit(0, false)]),
            ]),
            vec![VarId(1)],
        ),
        // all-free show over a SAT formula → 2^2 = 4 (empty constrained set).
        (
            crate::test_support::formula(3, vec![Clause::new(vec![lit(0, true)])]),
            vec![VarId(2), VarId(3)],
        ),
        // 3 constrained show, mixed: (x0∨x1∨x2)∧(¬x0∨¬x1) → 5.
        (
            crate::test_support::formula(3, vec![
                Clause::new(vec![lit(0, true), lit(1, true), lit(2, true)]),
                Clause::new(vec![lit(0, false), lit(1, false)]),
            ]),
            vec![VarId(1), VarId(2), VarId(3)],
        ),
    ]
}

/// Deterministic pseudo-random CNF, at most 3 literals per (deduplicated)
/// clause — the differential population, drawn through the library's
/// shared generator. Small enough for the brute-force oracle to answer
/// independently.
fn random_cnf(vars: u32, clauses: usize, seed: u64) -> CnfFormula {
    let mut rng = tididi::test_helpers::Lcg::new(seed);
    let shape = tididi::test_helpers::CnfShape { clauses, width: 3 };
    let raw = tididi::test_helpers::rand_cnf(&mut rng, vars, shape);
    crate::test_support::formula(
        vars,
        raw.iter().map(|c| crate::test_support::clause(c)).collect(),
    )
}

/// A differential check against an independent oracle, over the
/// micro-formulas and a pseudo-random CNF population rather than hand-picked
/// shapes alone. The blocking enumerator's two failure modes are silent in a
/// single-formula test — a projection blocked too widely disappears from the
/// tally, one blocked too narrowly is found twice — and both move the count
/// off `brute_projected_count`, which shares no code with the enumerator. The
/// 2^K sweep is checked against the same oracle on every case.
#[test]
fn projected_allsat_matches_the_oracle_on_a_random_population() {
    for (i, (f, show)) in micro_cases().iter().enumerate() {
        let oracle = brute_projected_count(f, show);
        let got = allsat(
            f,
            &split_show(f, show),
            1_000_000,
            Duration::from_secs(60),
            &mut AllsatCtl::default(),
            Models(0),
        );
        assert_eq!(got, Some(oracle.clone()), "micro {i}");
        assert_eq!(sweep(f, &split_show(f, show), Models(0), None), Some(oracle), "sweep, micro {i}");
    }
    // 10 vars keeps the oracle's 2^n sweep cheap; 6 show vars leave hidden
    // vars for the projection to quantify away.
    let show: Vec<VarId> = (1..=6).map(VarId).collect();
    for seed in 0..40u64 {
        let f = random_cnf(10, 14, seed);
        let oracle = brute_projected_count(&f, &show);
        let got = allsat(
            &f,
            &split_show(&f, &show),
            1_000_000,
            Duration::from_secs(60),
            &mut AllsatCtl::default(),
            Models(0),
        );
        assert_eq!(got, Some(oracle.clone()), "seed {seed}");
        assert_eq!(sweep(&f, &split_show(&f, &show), Models(0), None), Some(oracle), "sweep, seed {seed}");
    }
}

/// Completeness on a dense projection set — thousands of projections rather
/// than the handful every other case here has, which is the regime the
/// blocking loop actually runs in and the only one where an accumulation bug
/// has room to show. `(x0 ∨ … ∨ x13) ∧ (¬x0 ∨ ¬x1)` over 14 constrained show
/// vars admits every assignment except all-false and the quarter with
/// x0 = x1 = true, so the count is `2^14 − 1 − 2^12` — asserted against both
/// the closed form and the independent oracle.
#[test]
fn projected_allsat_is_complete_on_a_dense_projection_set() {
    const K: u32 = 14;
    let mut clauses = vec![Clause::new((0..K).map(|v| lit(v, true)).collect())];
    clauses.push(Clause::new(vec![lit(0, false), lit(1, false)]));
    let f = crate::test_support::formula(K, clauses);
    let show: Vec<VarId> = (1..=K).map(VarId).collect();
    let expected = BigUint::from((1u32 << K) - 1 - (1u32 << (K - 2)));

    assert_eq!(
        brute_projected_count(&f, &show),
        expected,
        "oracle disagrees"
    );
    assert_eq!(
        allsat(
            &f,
            &split_show(&f, &show),
            1_000_000,
            Duration::from_secs(60),
            &mut AllsatCtl::default(),
            Models(0),
        ),
        Some(expected.clone()),
    );
}
