use super::*;
use tididi::Engine;
use crate::test_support::{self as fixtures, clause};

#[test]
fn simplify_via_unit_drops_satisfied_and_shortens_falsified() {
    // F = (x0 ∨ x1) ∧ (¬x0 ∨ x2) ∧ (x1 ∨ x2)
    let f = fixtures::formula(3, vec![clause(&[1, 2]), clause(&[-1, 3]), clause(&[2, 3])]);
    // Force x0 = true → drop C0 (sat), shorten C1 to (x2) → derived unit
    // → C2 contains x2 (sat) → drop.
    let g = simplify_via_unit(&f, Literal::pos(VarId::from_dimacs(1))).expect("not UNSAT");
    // Remaining non-unit clauses: none. Unit-clauses-reattached: (x0), (x2).
    let unit_lits: Vec<Literal> = g
        .clauses()
        .iter()
        .filter(|c| c.literals.len() == 1)
        .map(|c| c.literals[0])
        .collect();
    assert!(unit_lits.contains(&Literal::pos(VarId::from_dimacs(1))));
    assert!(unit_lits.contains(&Literal::pos(VarId::from_dimacs(3))));
}

#[test]
fn simplify_via_unit_reports_unsat() {
    // F = (x0) ∧ (¬x0)
    let f = fixtures::formula(1, vec![clause(&[1]), clause(&[-1])]);
    // Force x0 = true → ¬x0 clause becomes empty → UNSAT.
    assert!(simplify_via_unit(&f, Literal::pos(VarId::from_dimacs(1))).is_none());
}

/// A cofactor compile that must not be reached.
fn no_cofactor(_: &Run<'_>, _: &CnfFormula) -> Option<Counted> {
    unreachable!("no cofactor is compiled here")
}

/// The first call's count and peak pass through, and it names the stage that
/// answered it, a stage of the caller's behind the first compile included.
#[test]
fn recovery_names_the_stage_the_first_call_ran() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(2, vec![clause(&[1])]);
    let first = || Some(((BigUint::from(2u32), 1024usize), AnsweredBy::StallRecompile));
    let got = count_with_recovery(&fixtures::run(eng, &config), &f, first, None, no_cofactor).unwrap();
    assert_eq!(
        (got.count, got.peak_pairs, got.answered_by),
        (BigUint::from(2u32), 1024, AnsweredBy::StallRecompile)
    );
}

/// A count a fallback produced is the fallback's.
#[test]
fn recovery_names_the_fallback_that_answers() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(2, vec![clause(&[1, 2])]);
    let first = || -> Option<(Counted, AnsweredBy)> { panic!("the first compile fails") };
    let fallbacks = || Some(((BigUint::from(3u32), 0usize), AnsweredBy::SupportEnumeration));
    let got = count_with_recovery(&fixtures::run(eng, &config), &f, first, Some(&fallbacks), no_cofactor).unwrap();
    assert_eq!((got.count, got.answered_by), (BigUint::from(3u32), AnsweredBy::SupportEnumeration));
}

/// A count the split assembled is the split's, also when no cofactor
/// reached a compile: here both are refuted by BCP, so the count is 0 and
/// only the first compile ran.
#[test]
fn recovery_names_the_split_when_no_cofactor_compiles() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(1, vec![clause(&[1]), clause(&[-1])]);
    let calls = std::cell::Cell::new(0u32);
    let first = || -> Option<(Counted, AnsweredBy)> {
        calls.set(calls.get() + 1);
        panic!("the first compile fails")
    };
    let got = count_with_recovery(&fixtures::run(eng, &config), &f, first, None, no_cofactor).unwrap();
    assert_eq!((got.count, got.answered_by), (BigUint::ZERO, AnsweredBy::ShannonSplit));
    assert_eq!(calls.get(), 1);
}

/// A compile that gives up without panicking while the run's wall is left
/// fails as a panicking one does: the formula is split, each cofactor is
/// counted as a cofactor's compile, and the sum is the formula's count.
#[test]
fn a_compile_that_gives_up_with_time_left_is_split() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(3, vec![clause(&[1, 2]), clause(&[-1, 3])]);
    let cofactor = |run: &Run<'_>, form: &CnfFormula| {
        assert!(run.compile.cofactor, "a cofactor compiles as a cofactor's compile");
        Some((fixtures::brute_projected_count(form, 0..form.num_vars() as usize), 0))
    };
    let got = count_with_recovery(&fixtures::run(eng, &config), &f, || None, None, cofactor).unwrap();
    assert_eq!(got.answered_by, AnsweredBy::ShannonSplit);
    assert_eq!(got.count, fixtures::brute_projected_count(&f, 0..3));
}

/// The cascade honors the run's wall. With it already past, the count
/// gives up (None) without ever invoking a compile or a Shannon split.
#[test]
fn mc_recovery_honors_past_deadline_without_compiling() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(3, vec![clause(&[1, 2])]);
    let past = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let run = Run { deadline: Some(past), ..fixtures::run(eng, &config) };
    let calls = std::cell::Cell::new(0u32);
    let first = || -> Option<(Counted, AnsweredBy)> {
        calls.set(calls.get() + 1);
        panic!("compile forced to fail — must not be reached past the deadline");
    };
    assert!(count_with_recovery(&run, &f, first, None, no_cofactor).is_none(), "past-deadline recovery must give up");
    assert_eq!(calls.get(), 0, "past-deadline recovery must not invoke the compile at all");
}

/// The fallbacks re-check the wall immediately before they start, rather
/// than relying on the check made before the first compile: a panicking
/// compile that burned the remaining budget (the OOM case this exists for)
/// must not admit a second full compile. The wall is a second ahead, so the
/// entry check passes and the first compile runs; it waits for the wall to
/// pass before panicking.
#[test]
fn mc_recovery_skips_the_fallbacks_once_the_wall_passes_mid_attempt() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(3, vec![clause(&[1, 2])]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let run = Run { deadline: Some(deadline), ..fixtures::run(eng, &config) };
    let compile_calls = std::cell::Cell::new(0u32);
    let first = || -> Option<(Counted, AnsweredBy)> {
        compile_calls.set(compile_calls.get() + 1);
        // An attempt (an OOM, say) that eats the rest of the wall before
        // dying.
        while !deadline_passed(Some(deadline)) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("compile forced to fail after the wall passed");
    };
    let fallbacks_called = std::cell::Cell::new(false);
    let fallbacks = || {
        fallbacks_called.set(true);
        Some(((BigUint::from(1u32), 0), AnsweredBy::UnreducedRetry))
    };
    let _ = count_with_recovery(&run, &f, first, Some(&fallbacks), no_cofactor);
    assert_eq!(compile_calls.get(), 1, "the first compile must run exactly once");
    assert!(
        !fallbacks_called.get(),
        "the fallbacks must not start once the wall has passed, \
         even though it was still in the future when the panicking attempt started"
    );
}
