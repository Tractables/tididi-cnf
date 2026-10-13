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

#[test]
fn recovery_passes_through_when_compile_succeeds() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(2, vec![clause(&[1])]);
    let first = || Some(((BigUint::from(42u32), 1024usize), AnsweredBy::Compile));
    let got = count_with_recovery(&fixtures::run(eng, &config), &f, first, None, no_cofactor).unwrap();
    assert_eq!((got.count, got.peak_pairs, got.answered_by), (BigUint::from(42u32), 1024, AnsweredBy::Compile));
}

/// The first call names the stage that answered it, a stage of the caller's
/// behind the first compile included.
#[test]
fn recovery_names_the_stage_the_first_call_ran() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    let f = fixtures::formula(2, vec![clause(&[1])]);
    let first = || Some(((BigUint::from(2u32), 0usize), AnsweredBy::StallRecompile));
    let got = count_with_recovery(&fixtures::run(eng, &config), &f, first, None, no_cofactor).unwrap();
    assert_eq!((got.count, got.answered_by), (BigUint::from(2u32), AnsweredBy::StallRecompile));
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

/// A cofactor the split re-reduces is numbered afresh, so the variables the
/// enclosing splits fixed name nothing in it: when it fails, it branches on
/// its own most frequent variable, whatever index the enclosing branch had.
#[test]
fn a_re_reduced_cofactor_branches_on_its_own_most_frequent_variable() {
    let eng = &Engine::new();
    let config = fixtures::minimal_config();
    // x1 and x2 tie as the most frequent variables, so the split branches on
    // x1, index 0. Under x1 every clause of x1 is satisfied, x2 is the most
    // frequent variable left, and the reduction numbers it index 0.
    let f = fixtures::formula(7, vec![
        clause(&[1, 3, 4]),
        clause(&[1, -5, 6]),
        clause(&[1, -3, 5]),
        clause(&[1, 4, -6]),
        clause(&[1, 5, 7]),
        clause(&[1, -4, -7]),
        clause(&[2, 3, -4]),
        clause(&[-2, 5, 6]),
        clause(&[2, -6, 7]),
        clause(&[-2, -3, -7]),
        clause(&[2, 4, 5]),
        clause(&[-2, -5, 3]),
    ]);
    let seen = std::cell::RefCell::new(Vec::new());
    // The first cofactor fails, so it is split in turn; the rest answer.
    let cofactor = |_: &Run<'_>, form: &CnfFormula| {
        seen.borrow_mut().push(form.clone());
        let first = seen.borrow().len() == 1;
        (!first).then(|| (fixtures::brute_projected_count(form, 0..form.num_vars() as usize), 0))
    };
    let got = count_with_recovery(&fixtures::run(eng, &config), &f, || None, None, cofactor).unwrap();
    assert_eq!(got.count, fixtures::brute_projected_count(&f, 0..7));
    let seen = seen.into_inner();
    let reduced = &seen[0];
    assert!(reduced.num_vars() < f.num_vars(), "the fixture's first cofactor is re-reduced");
    let top = most_frequent_var(reduced, 0, |_| true).expect("the reduced cofactor has clauses");
    assert_eq!(top.idx(), 0, "the fixture numbers the reduced cofactor's top variable as the enclosing branch");
    let children: Vec<CnfFormula> = [Literal::pos(top), Literal::neg(top)]
        .into_iter()
        .filter_map(|unit| simplify_via_unit(reduced, unit))
        .collect();
    assert_eq!(seen[1..=children.len()], children[..], "the reduced cofactor's split branches on its top variable");
}

/// The cascade honors the run's wall. With it already past, the count
/// gives up (None) without ever invoking a compile — no Shannon split, no
/// per-child re-Arjun.
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
