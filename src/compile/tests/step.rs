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

// The same pass with no deadline completes with the formula's diagram at
// the root — the pin that makes the give-up case above meaningful.
#[test]
fn uncut_pass_completes() {
    let tdd = run_pass(&Engine::new(), 3, &[&[1, 2], &[-2, 3]], None)
        .expect("an uncut pass completes");
    assert_eq!(tdd.model_count().unwrap(), 4u32.into(), "a completed pass leaves the result at the root");
}

/// A step whose satisfiability query the engine refuses gives the compile
/// up; it is never read as a proof of UNSAT.
#[test]
fn refused_step_query_aborts_without_claiming_unsat() {
    let eng = &Engine::new();
    let _deadline = eng.limits().edit(|limits| {
        limits.with_deadline(Some(std::time::Instant::now()))
    });
    assert!(run_pass(eng, 2, &[&[1]], None).is_none());
}

// A node proved UNSAT is a distinct outcome from an abort: the formula is
// proved UNSAT however much is left uncompiled, and the answer is ⊥.
#[test]
fn short_circuit_reports_unsat() {
    let eng = &Engine::new();
    let tdd = run_pass(eng, 3, &[&[1], &[-1]], None).expect("a proof of UNSAT is an answer");
    assert!(!eng.is_sat(&tdd).unwrap());
}
