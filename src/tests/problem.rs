use super::*;

use tididi::test_helpers::rat;

#[test]
fn a_count_lift_is_a_power_of_two_and_composes_by_adding() {
    assert_eq!(Unweighted::compose(&3, 4), 7);
    assert_eq!(
        Unweighted::lifted(num_bigint::BigUint::from(5u32), &3),
        num_bigint::BigUint::from(40u32)
    );
    assert_eq!(Unweighted::lifted(Unweighted::one(), &Unweighted::no_lift()), Unweighted::one());
}

#[test]
fn a_weight_lift_is_a_rational_and_composes_by_multiplying() {
    assert_eq!(Weighted::compose(&rat(1, 2), rat(3, 5)), rat(3, 10));
    assert_eq!(
        Weighted::lifted(WeightedCount::Satisfiable(rat(2, 1)), &rat(3, 4)),
        WeightedCount::Satisfiable(rat(3, 2))
    );
    assert_eq!(Weighted::lifted(Weighted::one(), &Weighted::no_lift()), Weighted::one());
}

/// A zero lift, from a variable weighed zero both ways, makes the count of
/// a satisfiable problem zero and leaves it satisfiable; an unsatisfiable
/// problem stays unsatisfiable under any lift.
#[test]
fn a_weight_lift_keeps_the_verdict() {
    assert_eq!(Weighted::lifted(Weighted::one(), &rat(0, 1)), WeightedCount::Satisfiable(rat(0, 1)));
    assert_eq!(Weighted::lifted(Weighted::zero(), &rat(3, 4)), WeightedCount::Unsatisfiable);
}

#[test]
fn a_rewrite_composes_the_lifts_and_takes_the_rest_from_the_reduction() {
    let raw = crate::test_support::formula(2, vec![]);
    let reduced = crate::test_support::formula(1, vec![]);
    let show = |ids: &[u32]| Some(ShowSet::from_dimacs_ids(ids).expect("variables from 1"));
    let first: Problem<Unweighted> = Problem { formula: raw, show: show(&[1, 2]), weights: (), lift: 2 };
    let next: Problem<Unweighted> = Problem { formula: reduced, show: show(&[1]), weights: (), lift: 5 };
    let problem = first.rewritten(next);
    assert_eq!(problem.lift, 7);
    assert_eq!(problem.formula.num_vars(), 1);
    assert_eq!(problem.show, show(&[1]));
}

/// Variable 2 is given only its positive literal, at 1/4, so its negative
/// literal weighs 1 - 1/4; variables 1 and 3, given nothing, weigh 1 both
/// ways.
#[test]
fn a_declared_problem_reads_the_show_set_and_resolves_unspecified_weights_by_the_competition_rule() {
    let formula = crate::test_support::formula(3, vec![]);
    let show = vitri::cnf::ShowSet::<vitri::cnf::Original>::from_vars(
        [2usize, 0].into_iter().map(vitri::cnf::VarId::from_idx),
    );
    let weights = vitri::cnf::WeightTable::from_dimacs_pairs([(2, rat(1, 4))], 3)
        .expect("one literal of a three-variable formula");
    let meta = vitri::cnf::CnfMeta::from_parts(
        3,
        Some(vitri::cnf::Mode::Pwmc),
        Some(show),
        Some(weights),
    )
    .expect("a projected weighted declaration");
    let problem: Problem<Weighted> = Problem::declared(formula, &meta);
    assert_eq!(problem.show.as_ref().map(ShowSet::as_dimacs), Some(&[1, 3][..]));
    assert_eq!(problem.lift, rat(1, 1));
    assert_eq!(
        problem.weights,
        vec![
            (rat(1, 1), rat(1, 1)),
            (rat(3, 4), rat(1, 4)),
            (rat(1, 1), rat(1, 1)),
        ]
    );
}

/// A show set crosses into the problem's own space with the same variables.
#[test]
fn independent_support_crosses_without_changing_variable_ids() {
    let support = vitri::cnf::ShowSet::<vitri::cnf::Reduced>::from_vars(
        [2, 5, 9].map(vitri::cnf::VarId::from_dimacs),
    );
    assert_eq!(problem_show(&support).as_dimacs(), [2, 5, 9]);
    let empty = vitri::cnf::ShowSet::<vitri::cnf::Reduced>::empty();
    assert!(problem_show(&empty).is_empty());
}
