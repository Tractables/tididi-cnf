/// A cofactor is counted in its own space whatever its preprocessing does:
/// the lift of the variables it settles is folded into a compiled count, a
/// cofactor it settles outright counts by the lift alone, and one it refutes
/// counts 0.
#[test]
fn a_cofactor_is_counted_in_its_own_space() {
    use crate::test_support::{brute_projected_count, dimacs};
    let eng = &tididi::Engine::new();
    let config = crate::test_support::minimal_config();
    let run = crate::test_support::run(eng, &config);
    let cofactors = [
        // x1 pinned as a split leaves it, x5 free, a core to compile.
        dimacs(5, &[&[1][..], &[2, 3], &[-2, 4], &[3, -4]]),
        // Every variable pinned or free.
        dimacs(3, &[&[1], &[-2]]),
        // No unit, and no model.
        dimacs(2, &[&[1, 2], &[1, -2], &[-1, 2], &[-1, -2]]),
    ];
    for cofactor in &cofactors {
        let (count, _) = super::count_cofactor(&run, cofactor, super::Route::Reduced).expect("a small cofactor counts");
        assert_eq!(count, brute_projected_count(cofactor, 0..cofactor.num_vars() as usize));
    }
}

/// Count the product of `n` by `n` positive clauses `a ∨ x`, monotone, which
/// a model count skips Arjun on, through the monotone reduction alone: the count, in
/// the input's space, and the stage that answered.
fn count_a_monotone_product(n: i32) -> (num_bigint::BigUint, Option<(num_bigint::BigUint, crate::output::AnsweredBy)>) {
    use crate::test_support::dimacs;
    use std::time::{Duration, Instant};
    use vitri::bundle::{SkipReason, StageOutcome};

    let clauses: Vec<[i32; 2]> = (1..=n).flat_map(|a| (n + 1..=2 * n).map(move |x| [a, x])).collect();
    let formula = dimacs(2 * n as u32, &clauses);
    let want = crate::test_support::brute_projected_count(&formula, 0..2 * n as usize);
    let eng = &tididi::Engine::new();
    let config = crate::config::Config {
        mode: crate::config::Mode::Mc,
        preprocess_level: crate::config::PreprocessLevel::Full,
        time_limit_ms: Some(3_600_000),
        ..crate::test_support::minimal_config()
    };
    let deadline = Some(Instant::now() + Duration::from_secs(3600));
    let run = crate::stage::Run { deadline, ..crate::test_support::run(eng, &config) };
    let counted = crate::frontend::with_mc_frontend(&formula, &config, deadline, crate::frontend::TreeFor::MainCompile, |attempt, session| {
        let bundle = attempt.preprocessed;
        assert_eq!(bundle.stages.arjun, Some(StageOutcome::Skipped(SkipReason::Monotone)));
        let simplify_pow2 = bundle.count_lift.simplify_pow2;
        super::monotone_reduction(&run, &bundle.reduced, session, false)
            .map(|((count, _), answered_by)| (count << simplify_pow2, answered_by))
    })
    .expect("vitri's frontend runs");
    (want, counted)
}

/// A model count skips Arjun on a monotone formula, and the monotone reduction asks
/// the frontend session for the reduction it skipped. Bounded variable
/// addition shortens a six by six product, and the compile of the reduction
/// answers: the count is the formula's, through the reduction's lift.
#[test]
fn the_monotone_reduction_compiles_the_reduction_arjun_skipped() {
    let (want, counted) = count_a_monotone_product(6);
    assert_eq!(counted, Some((want, crate::output::AnsweredBy::MonotoneReduction)));
}

/// Arjun leaves a four by four product as it is, and a reduction no smaller
/// than the formula is not compiled again: support enumeration over its
/// independent support answers instead.
#[test]
fn a_reduction_no_smaller_than_the_formula_is_not_compiled_again() {
    let (want, counted) = count_a_monotone_product(4);
    assert_eq!(counted, Some((want, crate::output::AnsweredBy::SupportEnumeration)));
}

/// The retry a compile that ran out of memory gets first: under the byte
/// budget the plain compile's retry is tested under, where the compile on the
/// linear vtree runs out of memory, the bisection counts
/// `(x_1 ∨ … ∨ x_n) ∧ ⋀_i (x_i ↔ y_i)` exactly, through the reduction's lift.
/// It does not run under an explicit vtree, nor when the portfolio already
/// picked a bisection.
#[test]
fn a_reduction_out_of_memory_is_counted_on_the_bisection() {
    const N: i32 = 24;
    const LIFT: u32 = 3;
    let mut clauses = vec![(1..=N).collect::<Vec<_>>()];
    for i in 1..=N {
        clauses.push(vec![-i, N + i]);
        clauses.push(vec![i, -(N + i)]);
    }
    let ar = crate::frontend::record::ArjunReduction {
        formula: crate::test_support::dimacs(2 * N as u32, &clauses),
        lift_pow2: LIFT,
        support: crate::problem::ShowSet::from_vars(std::iter::empty()),
    };
    let context = std::sync::Arc::new(tididi::Context::new());
    context.run(|eng| {
        let _budget = eng.limits().edit(|limits| limits.with_memory_budget_bytes(Some(4 << 20)));
        let config = crate::test_support::minimal_config();
        let run = crate::test_support::run(eng, &config);
        let (count, _) =
            super::bisect_retry(&run, &ar, Some("linear"), None).expect("the bisection compiles under the budget");
        assert_eq!(count, ((num_bigint::BigUint::from(1u32) << N as usize) - 1u32) << LIFT as usize);
        assert!(super::bisect_retry(&run, &ar, Some(crate::stage::RETRY_VTREE_SPEC), None).is_none());
        let explicit = crate::config::Config { vtree: Some("linear".to_owned()), ..crate::test_support::minimal_config() };
        assert!(super::bisect_retry(&crate::test_support::run(eng, &explicit), &ar, Some("linear"), None).is_none());
    });
}

/// The unreduced retry is skipped only when the reduction is the formula: the
/// same variables occur, and at most 1% of the clauses went, none came.
#[test]
fn a_reduction_that_removed_nothing_is_the_same_formula() {
    let dimacs = crate::test_support::dimacs;
    let clauses: Vec<Vec<i32>> = (1..=200).map(|i| vec![i % 50 + 1, -((i * 7 + 1) % 50 + 1)]).collect();
    let refs: Vec<&[i32]> = clauses.iter().map(Vec::as_slice).collect();
    let form = dimacs(50, &refs);
    assert!(super::the_same_formula(&form, &form));
    assert!(super::the_same_formula(&dimacs(50, &refs[1..]), &form), "a dropped duplicate");
    assert!(!super::the_same_formula(&dimacs(50, &refs[10..]), &form), "5% of the clauses went");
    let fewer: Vec<&[i32]> = refs.iter().copied().filter(|c| c.iter().all(|l| l.unsigned_abs() != 1)).collect();
    assert!(!super::the_same_formula(&dimacs(50, &fewer), &form), "a variable went");
}
