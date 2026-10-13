use super::*;
use crate::problem::Unweighted;
use std::cell::RefCell;

struct Run {
    calls: RefCell<Vec<&'static str>>,
}

fn problem(num_vars: u32, lift: u32) -> Problem<Unweighted> {
    Problem {
        formula: crate::test_support::formula(num_vars, vec![]),
        show: None,
        weights: (),
        lift,
    }
}

fn declines(run: &Run, _: &Problem<Unweighted>, _: &Trace) -> Step<Unweighted> {
    run.calls.borrow_mut().push("declines");
    Step::Decline
}

fn halves(run: &Run, p: &Problem<Unweighted>, _: &Trace) -> Step<Unweighted> {
    run.calls.borrow_mut().push("halves");
    Step::Rewrite(problem(p.formula.num_vars() / 2, 1))
}

fn answers_three(run: &Run, _: &Problem<Unweighted>, _: &Trace) -> Step<Unweighted> {
    run.calls.borrow_mut().push("answers");
    Step::Answer(num_bigint::BigUint::from(3u32))
}

fn answers_the_trace(run: &Run, p: &Problem<Unweighted>, trace: &Trace) -> Step<Unweighted> {
    run.calls.borrow_mut().push("trace");
    assert_eq!(trace.input_vars, 8);
    assert_eq!(trace.input_clauses, 0);
    assert_eq!(p.formula.num_vars(), 2, "two halvings reached this stage");
    Step::Answer(num_bigint::BigUint::from(1u32))
}

type Fixture = fn(&Run, &Problem<Unweighted>, &Trace) -> Step<Unweighted>;

/// [`run_stages`] over `fixtures`, each a stage on `run`.
fn run_fixtures(
    run: &Run,
    problem: Problem<Unweighted>,
    fixtures: &[Fixture],
    deadline: Option<Instant>,
) -> Result<Projected<num_bigint::BigUint>, crate::Error> {
    let stages: Vec<_> = fixtures
        .iter()
        .map(|&fixture| move |problem: &Problem<Unweighted>, trace: &Trace| fixture(run, problem, trace))
        .collect();
    let stages: Vec<Stage<'_, Unweighted>> = stages.iter().map(|stage| stage as Stage<'_, Unweighted>).collect();
    run_stages(problem, &stages, deadline, &crate::budget::VtreeTime::default())
}

#[test]
fn the_first_answer_ends_the_run_and_carries_the_lift() {
    let run = Run { calls: RefCell::new(Vec::new()) };
    let got = run_fixtures(&run, problem(4, 2), &[declines, answers_three, declines], None);
    assert_eq!(got.ok().map(|answer| answer.value), Some(num_bigint::BigUint::from(12u32)));
    assert_eq!(*run.calls.borrow(), ["declines", "answers"]);
}

#[test]
fn every_stage_declining_is_no_answer() {
    let run = Run { calls: RefCell::new(Vec::new()) };
    assert!(run_fixtures(&run, problem(4, 0), &[declines, declines], None).is_err());
    assert_eq!(*run.calls.borrow(), ["declines", "declines"]);
}

#[test]
fn an_expired_deadline_does_not_start_another_stage() {
    let run = Run { calls: RefCell::new(Vec::new()) };
    let deadline = Instant::now() - std::time::Duration::from_secs(1);
    assert!(run_fixtures(&run, problem(4, 0), &[answers_three], Some(deadline)).is_err());
    assert!(run.calls.borrow().is_empty());
}

#[test]
fn a_rewrite_composes_its_lift_into_the_answer_and_reaches_the_next_stage() {
    let run = Run { calls: RefCell::new(Vec::new()) };
    let got = run_fixtures(&run, problem(8, 1), &[halves, halves, answers_the_trace], None);
    // 1 << (1 + 1 + 1)
    assert_eq!(got.ok().map(|answer| answer.value), Some(num_bigint::BigUint::from(8u32)));
    assert_eq!(*run.calls.borrow(), ["halves", "halves", "trace"]);
}
