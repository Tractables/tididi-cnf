use super::*;
use crate::problem::Unweighted;
use std::cell::RefCell;

/// The stages that ran, in order.
#[derive(Default)]
struct Calls(RefCell<Vec<&'static str>>);

fn problem(num_vars: u32, lift: u32) -> Problem<Unweighted> {
    Problem {
        formula: crate::test_support::formula(num_vars, vec![]),
        show: None,
        weights: (),
        lift,
    }
}

fn declines(calls: &Calls, _: &Problem<Unweighted>, _: &Trace) -> Step<Unweighted> {
    calls.0.borrow_mut().push("declines");
    Step::Decline
}

fn halves(calls: &Calls, p: &Problem<Unweighted>, _: &Trace) -> Step<Unweighted> {
    calls.0.borrow_mut().push("halves");
    Step::Rewrite(problem(p.formula.num_vars() / 2, 1))
}

fn answers_three(calls: &Calls, _: &Problem<Unweighted>, _: &Trace) -> Step<Unweighted> {
    calls.0.borrow_mut().push("answers");
    Step::Answer(num_bigint::BigUint::from(3u32))
}

fn answers_the_trace(calls: &Calls, p: &Problem<Unweighted>, trace: &Trace) -> Step<Unweighted> {
    calls.0.borrow_mut().push("trace");
    assert_eq!(trace.input_vars, 8);
    assert_eq!(trace.input_clauses, 0);
    assert_eq!(p.formula.num_vars(), 2, "two halvings reached this stage");
    Step::Answer(num_bigint::BigUint::from(1u32))
}

type Fixture = fn(&Calls, &Problem<Unweighted>, &Trace) -> Step<Unweighted>;

/// [`run_stages`] over `fixtures`, each a stage logging to `calls`.
fn run_fixtures(
    calls: &Calls,
    problem: Problem<Unweighted>,
    fixtures: &[Fixture],
    deadline: Option<Instant>,
) -> Result<Projected<num_bigint::BigUint>, crate::Error> {
    let stages: Vec<_> = fixtures
        .iter()
        .map(|&fixture| move |problem: &Problem<Unweighted>, trace: &Trace| fixture(calls, problem, trace))
        .collect();
    let stages: Vec<Stage<'_, Unweighted>> = stages.iter().map(|stage| stage as Stage<'_, Unweighted>).collect();
    run_stages(problem, &stages, deadline, &crate::budget::VtreeTime::default())
}

#[test]
fn the_first_answer_ends_the_run_and_carries_the_lift() {
    let calls = Calls::default();
    let got = run_fixtures(&calls, problem(4, 2), &[declines, answers_three, declines], None);
    assert_eq!(got.ok().map(|answer| answer.value), Some(num_bigint::BigUint::from(12u32)));
    assert_eq!(*calls.0.borrow(), ["declines", "answers"]);
}

#[test]
fn every_stage_declining_is_no_answer() {
    let calls = Calls::default();
    assert!(run_fixtures(&calls, problem(4, 0), &[declines, declines], None).is_err());
    assert_eq!(*calls.0.borrow(), ["declines", "declines"]);
}

#[test]
fn an_expired_deadline_does_not_start_another_stage() {
    let calls = Calls::default();
    let deadline = Instant::now() - std::time::Duration::from_secs(1);
    assert!(run_fixtures(&calls, problem(4, 0), &[answers_three], Some(deadline)).is_err());
    assert!(calls.0.borrow().is_empty());
}

#[test]
fn a_rewrite_composes_its_lift_into_the_answer_and_reaches_the_next_stage() {
    let calls = Calls::default();
    let got = run_fixtures(&calls, problem(8, 1), &[halves, halves, answers_the_trace], None);
    // 1 << (1 + 1 + 1)
    assert_eq!(got.ok().map(|answer| answer.value), Some(num_bigint::BigUint::from(8u32)));
    assert_eq!(*calls.0.borrow(), ["halves", "halves", "trace"]);
}

/// The projected Arjun budget takes a size-adaptive share of the declared
/// wall: 38%, up to the 300 s ceiling, and 50% on a formula of a million
/// variables or more.
#[test]
fn the_projected_arjun_reduce_takes_a_size_adaptive_share_of_the_wall() {
    assert_eq!(projected_arjun_budget_ms(Some(3_600_000), 1_000), 300_000);
    assert_eq!(projected_arjun_budget_ms(Some(120_000), 1_000), 45_600);
    assert_eq!(projected_arjun_budget_ms(Some(120_000), 2_000_000), 60_000);
    // No wall declared ⇒ the flat default.
    assert_eq!(projected_arjun_budget_ms(None, 1_000), 300_000);
}

/// Even the largest declared wall reaches the budget ceiling without overflow.
#[test]
fn the_projected_arjun_budget_caps_the_largest_time_limit() {
    for num_vars in [1_000, 2_000_000] {
        assert_eq!(projected_arjun_budget_ms(Some(u64::MAX), num_vars), 300_000);
    }
}
