use super::*;

fn est(n_hat: f64) -> PathEstimate {
    PathEstimate { weights: vec![n_hat], ..PathEstimate::default() }
}

/// No path completed — the estimate is absent, not zero.
fn est_unmeasured() -> PathEstimate {
    PathEstimate { truncated: ESTIMATOR_MAX_TRUNCATED, ..PathEstimate::default() }
}

#[test]
fn declines_without_a_support() {
    assert_eq!(
        plan_support_enum(0, Some(3_000_000)),
        Err("Arjun reported no independent support"),
    );
}

#[test]
fn declines_on_an_oversized_support() {
    assert_eq!(
        plan_support_enum(MAX_SUPPORT + 1, Some(3_000_000)),
        Err("independent support larger than the enumeration band"),
    );
    // The ceiling itself is admitted.
    assert!(plan_support_enum(MAX_SUPPORT, Some(3_000_000)).is_ok());
}

#[test]
fn declines_without_a_wall() {
    assert_eq!(
        plan_support_enum(400, None),
        Err("no time limit to budget against"),
    );
}

#[test]
fn declines_when_the_window_is_too_short() {
    // 1_000_000 ms left ⇒ reserve max(300k, 250k) = 300k ⇒ wall 700k: admitted.
    assert_eq!(
        plan_support_enum(400, Some(1_000_000)),
        Ok(700_000),
    );
    // 899_999 ms left ⇒ reserve 300k ⇒ wall 599_999 < 600k: declined.
    assert_eq!(
        plan_support_enum(400, Some(899_999)),
        Err("too little wall left for a worthwhile enumeration"),
    );
    // The exact boundary is admitted.
    assert_eq!(
        plan_support_enum(400, Some(900_000)),
        Ok(600_000),
    );
}

#[test]
fn reserve_is_the_larger_of_the_floor_and_a_quarter() {
    // Below 1_200_000 ms the absolute floor binds…
    assert_eq!(plan_support_enum(400, Some(1_200_000)), Ok(900_000));
    // …above it, the proportional quarter does.
    assert_eq!(plan_support_enum(400, Some(2_600_000)), Ok(1_950_000));
    // A full 1h wall still leaves recovery a quarter.
    assert_eq!(plan_support_enum(400, Some(3_600_000)), Ok(2_700_000));
}

// ── what may bound a committed enumeration ──────────────────────────────

/// `commit_decision`'s five outcomes — no wall, UNSAT, unmeasured, over the
/// commit threshold, and a probe tally at or above [`TALLY_CEILING`] — each
/// as the `(estimate, available_ms, tallied)` triple that produces it, plus
/// the threshold and the tally ceiling at their own boundary.
#[test]
fn commit_decision_admits_or_refuses_by_outcome() {
    // label, estimate, available_ms, tallied, expected.
    type Row = (&'static str, PathEstimate, u64, u64, Result<u64, &'static str>);
    let unsat = || PathEstimate { unsat: true, ..est_unmeasured() };
    let rows: Vec<Row> = vec![
        ("no wall left", est(10.0), 0, 0, Err("no stage wall left to commit")),
        ("no wall left precedes even UNSAT", unsat(), 0, 0, Err("no stage wall left to commit")),
        ("UNSAT commits to finish its two solves", unsat(), 2_000_000, 0, Ok(2_000_000)),
        (
            "unmeasured bails",
            est_unmeasured(),
            2_000_000,
            0,
            Err("no sampled path completed — the count could not be estimated"),
        ),
        ("a measured zero commits", est(0.0), 2_000_000, 0, Ok(2_000_000)),
        ("at the threshold commits", est(COMMIT_MAX_MODELS), 2_000_000, 0, Ok(2_000_000)),
        (
            "just past the threshold bails",
            est(COMMIT_MAX_MODELS * 1.000_001),
            2_000_000,
            0,
            Err("estimated projected count above the commit threshold"),
        ),
        ("one short of the tally ceiling commits", est(500_000.0), 2_000_000, TALLY_CEILING - 1, Ok(2_000_000)),
        (
            "at the tally ceiling bails",
            est(500_000.0),
            2_000_000,
            TALLY_CEILING,
            Err("the probe already tallied past the count the gate admits"),
        ),
    ];
    for (label, estimate, available_ms, tallied, expected) in rows {
        assert_eq!(commit_decision(&estimate, available_ms, tallied), expected, "{label}");
    }
}

// ── the entry gate is the exit gate ─────────────────────────────────────

/// The entry ask and the exit ask are one call on one estimate, differing
/// only in the two probe-dependent arguments — so the entry gate can never
/// turn away a candidate the exit gate would have admitted, which is the
/// whole reason it is safe to ask before paying for the probe.
///
/// Pinned as an implication over the estimate shapes the policy distinguishes
/// rather than by listing verdicts, because the property that must hold is
/// the ordering of the two answers, not either one of them.
#[test]
fn the_entry_gate_never_refuses_what_the_exit_gate_would_admit() {
    let estimates = [
        est(1.0),
        est(COMMIT_MAX_MODELS),
        est(COMMIT_MAX_MODELS * 1.000_001),
        // An estimate enormously past the threshold, which the entry gate
        // must refuse without buying a probe first.
        est(7.393e101),
        est_unmeasured(),
        PathEstimate {
            unsat: true,
            ..est_unmeasured()
        },
    ];
    for estimate in estimates {
        // How `run_support_enum` asks at entry: the stage's whole wall (which
        // `plan_support_enum` has already floored at `MIN_ENUM_WALL_MS`) and
        // nothing tallied.
        let entry = commit_decision(&estimate, MIN_ENUM_WALL_MS, 0);
        for tallied in [0, 1, TALLY_CEILING - 1, TALLY_CEILING] {
            for available_ms in [1, MIN_ENUM_WALL_MS] {
                let exit = commit_decision(&estimate, available_ms, tallied);
                assert!(
                    entry.is_ok() || exit.is_err(),
                    "entry refused {estimate:?} the exit admits \
                     (available_ms={available_ms}, tallied={tallied})",
                );
            }
        }
    }
}

// ── end-to-end through the shared fast path ─────────────────────────────

/// Monotone chain `x0 → x1 → … → x(k-1)` plus one functionally-defined
/// extra var `x(k) ≡ x0 ∧ x(k-1)`. The chain admits exactly `k+1`
/// assignments (a false prefix then a true suffix), and the defined var adds
/// no freedom — so the formula has exactly `k+1` models and `{x0..x(k-1)}`
/// is an independent support of it, the same shape the stage consumes from
/// Arjun. With `k > 16` the cheap 2^K sweep declines and the projected
/// AllSAT band (the path the stage actually relies on) runs.
fn chain_with_defined_var(k: u32) -> (CnfFormula, ShowSet) {
    use vitri::cnf::{Clause, Literal};
    let lit = |v: u32, pos: bool| Literal::new(vitri::cnf::VarId::new(v + 1).unwrap(), pos);
    let mut clauses: Vec<Clause> = (0..k - 1)
        .map(|i| Clause::new(vec![lit(i, false), lit(i + 1, true)]))
        .collect();
    // x(k) ≡ x0 ∧ x(k-1)
    clauses.push(Clause::new(vec![
        lit(0, false),
        lit(k - 1, false),
        lit(k, true),
    ]));
    clauses.push(Clause::new(vec![lit(0, true), lit(k, false)]));
    clauses.push(Clause::new(vec![lit(k - 1, true), lit(k, false)]));
    (
        crate::test_support::formula(k + 1, clauses),
        ShowSet::from_vars((0..k as usize).map(vitri::cnf::VarId::from_idx)),
    )
}

#[test]
fn enumerates_the_support_exactly_through_the_shared_fastpath() {
    let (f, support) = chain_with_defined_var(20);
    // 20 support vars ⇒ past the 2^K sweep's ceiling ⇒ projected AllSAT.
    let count = run_support_enum(&f, &support, 60_000, 60_000)
        .expect("the enumeration fits both caps");
    assert_eq!(count, num_bigint::BigUint::from(21u32));
}

/// A probe wall too short to finish forces the estimate-then-commit path, and
/// the resumed enumeration still lands on the exact count.
#[test]
fn resumes_past_a_busted_probe_to_the_same_exact_count() {
    let (f, support) = chain_with_defined_var(20);
    let count = run_support_enum(&f, &support, 0, 60_000)
        .expect("a 0ms probe must estimate, commit and finish");
    assert_eq!(count, num_bigint::BigUint::from(21u32));
}

#[test]
fn declines_when_the_stage_wall_trips() {
    let (f, support) = chain_with_defined_var(20);
    // No probe wall and no stage wall ⇒ the estimator's commit cannot fit ⇒
    // bail without enumerating the rest.
    assert_eq!(run_support_enum(&f, &support, 0, 0), None);
}

/// The engine's generator starts at `seed | 1`: the paths' seeds must stay
/// distinct through it, or two paths walk the same way.
#[test]
fn path_seeds_give_distinct_generator_states() {
    let states: std::collections::HashSet<u64> =
        (0..ESTIMATOR_SAMPLES).map(|k| path_seed(k) | 1).collect();
    assert_eq!(states.len() as u32, ESTIMATOR_SAMPLES);
    // Path 0 keeps the base seed.
    assert_eq!(path_seed(0), 0x9E37_79B9_7F4A_7C15);
}

/// The estimator must land in the right universe on a formula whose
/// feasible-projection count is known exactly: the chain has k+1 of them.
///
/// A random-restriction estimator would return 0 here — 21 feasible
/// projections spread over 2^12 restrictions leaves almost every sample empty.
/// A chain is nearly all forced choices, which is precisely the shape a path
/// estimate handles and a restriction cannot.
#[test]
fn estimator_recovers_a_known_projection_count() {
    use tididi::vtree::VarId;
    let (f, support) = chain_with_defined_var(20);
    let show_vec: Vec<VarId> = crate::enumeration::show_vars(&support);
    let e = estimate_projected_count(&f, &show_vec);
    assert_eq!(e.weights.len() as u32, ESTIMATOR_SAMPLES, "{e:?}");
    assert_eq!(e.truncated, 0, "{e:?}");
    assert!(!e.unsat, "{e:?}");
    // True count 21. Path weights are powers of two, so no sample can be 21;
    // the mean has to bracket it and must never collapse to 0.
    let n_hat = e.mean().expect("every path completed");
    assert!((8.0..=64.0).contains(&n_hat), "{e:?}");
}
