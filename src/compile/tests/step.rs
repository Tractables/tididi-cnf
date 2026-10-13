use super::*;
use crate::compile::component::PassOutput;
use tididi::Engine;
use crate::test_support::{self as fixtures, clause};

/// The Boolean compile of the DIMACS `clauses` on a balanced vtree of
/// `num_vars` leaves.
fn run_pass(
    eng: &Engine,
    num_vars: u32,
    clauses: &[&[i32]],
    deadline: Option<std::time::Instant>,
) -> Option<Tdd> {
    let formula = fixtures::formula(num_vars, clauses.iter().map(|c| clause(c)).collect());
    let vtree = Arc::new(Vtree::balanced(num_vars));
    let job = CompileJob {
        deadline,
        output: PassOutput::Function,
        run: Default::default(),
    };
    compile_component(eng, &formula, &vtree, job).ok().map(|compiled| compiled.diagram)
}

/// A deadline that has passed cuts the pass at its first step.
#[test]
fn a_passed_deadline_aborts_the_pass() {
    let deadline = std::time::Instant::now();
    while !crate::budget::deadline_passed(Some(deadline)) {}
    assert!(run_pass(&Engine::new(), 2, &[&[1, 2]], Some(deadline)).is_none());
}

/// The same pass with no deadline completes with the formula's diagram at
/// the root — the pin that makes the give-up case above meaningful.
#[test]
fn uncut_pass_completes() {
    let tdd = run_pass(&Engine::new(), 3, &[&[1, 2], &[-2, 3]], None)
        .expect("an uncut pass completes");
    assert_eq!(tdd.model_count().unwrap(), 4u32.into(), "a completed pass leaves the result at the root");
}

/// An engine whose own deadline has passed refuses the first step's diagram
/// operation, and the compile gives up; the refusal is never read as a proof
/// of UNSAT.
#[test]
fn an_engine_stop_aborts_the_pass() {
    let eng = &Engine::new();
    let _deadline = eng.limits().edit(|limits| {
        limits.with_deadline(Some(std::time::Instant::now()))
    });
    assert!(run_pass(eng, 2, &[&[1]], None).is_none());
}

/// A node proved UNSAT is a distinct outcome from an abort: the formula is
/// proved UNSAT however much is left uncompiled, and the answer is ⊥.
#[test]
fn short_circuit_reports_unsat() {
    let eng = &Engine::new();
    let tdd = run_pass(eng, 3, &[&[1], &[-1]], None).expect("a proof of UNSAT is an answer");
    assert!(!eng.is_sat(&tdd).unwrap());
}

/// A pass that proves its formula UNSAT has answered, deadline or not: the
/// compile returns ⊥, not a give-up.
#[test]
fn a_proof_of_unsat_under_a_deadline_is_an_answer() {
    let eng = &Engine::new();
    let formula = fixtures::formula(2, vec![clause(&[1, 2]), clause(&[-1]), clause(&[-2])]);
    let vtree = Arc::new(Vtree::balanced(2));
    let deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(3600));
    for output in [crate::compile::Output::Function, crate::compile::Output::Count] {
        let opts = crate::compile::CompileOptions {
            deadline,
            output,
            plan: crate::compile::ComponentPlan::Whole,
        };
        let tdd = crate::compile::compile_cnf(eng, &formula, &vtree, opts)
            .expect("a proof of UNSAT is an answer, not a give-up")
            .diagram;
        assert!(!eng.is_sat(&tdd).unwrap());
    }
}
