use super::*;
use tididi::limits::StopAt;

#[test]
fn run_deadline_stays_armed_through_final_count_readout() {
    let eng = Engine::new();
    let formula = crate::parse_dimacs("p cnf 3 1\n1 2 3 0\n".as_bytes()).unwrap().0;
    let crate::vtree::VtreeBuild { root: tree, components } = crate::vtree::build_vtree_with_components(
        &formula, "portfolio", &crate::vtree::VtreeOptions::default(),
    ).unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    let value = with_run_deadline(&eng, Some(deadline), || {
        let options = CompileOptions {
            plan: ComponentPlan::from_components(components.as_deref()),
            ..CompileOptions::default()
        };
        let diagram = crate::compile::compile_cnf(&eng, &formula, &tree, options)?.diagram;
        assert_eq!(eng.limits().armed().stop_rules().unconditional, Some(StopAt::Time(deadline)));
        eng.model_count(&diagram).map_err(Error::stage_failed("count readout"))
    }).unwrap();
    assert_eq!(value, num_bigint::BigUint::from(7u32));
    assert_eq!(eng.limits().armed().stop_rules().unconditional, None);
}

/// A count readout the batch limits refuse ends the run as a refused compile
/// does: an incomplete run, not a failed artifact.
#[test]
fn a_refused_count_readout_is_an_incomplete_run() {
    let eng = Engine::new();
    let config = Config {
        preprocess_level: crate::config::PreprocessLevel::Full,
        ..crate::test_support::minimal_config()
    };
    // Preprocessing refutes the formula, so the readout is the first library
    // call the route makes, and the stop already passed refuses it.
    let formula = crate::parse_dimacs("p cnf 1 2\n1 0\n-1 0\n".as_bytes()).unwrap().0;
    let _stopped = crate::budget::deadline_scope(&eng, Some(Instant::now() - std::time::Duration::from_secs(1)));
    let error = run_function_compile(&crate::test_support::run(&eng, &config), &formula)
        .err()
        .expect("the readout was refused");
    assert!(matches!(error, Error::Incomplete(_)), "{error:?}");
}

/// The retry's construction is one vitri accepts, of the family the retry
/// checks the portfolio's pick against.
#[test]
fn the_retry_vtree_is_a_vitri_spec_of_its_family() {
    vitri::spec::validate_vtree_spec(RETRY_VTREE_SPEC)
        .expect("the retry construction must stay inside vitri's spec grammar");
    assert_eq!(spec_family(RETRY_VTREE_SPEC), "hypergraph-bisect");
}

/// The retry needs a vtree the default portfolio picked, of another family
/// than the retry's, and time left.
#[test]
fn the_retry_needs_the_default_portfolio_another_family_and_time_left() {
    let now = Instant::now();
    let later = Some(now + std::time::Duration::from_secs(60));
    let pick = Some("goatd-incidence:candidate=1");
    assert!(bisect_retry_applies(false, pick, later, now));
    assert!(bisect_retry_applies(false, pick, None, now), "a run with no time limit has time left");
    assert!(bisect_retry_applies(false, None, later, now), "no portfolio ran");
    assert!(
        bisect_retry_applies(false, Some("flowcutter-primal:binarize=hypergraph-bisect"), later, now),
        "a parameter naming the family does not make the vtree a bisection",
    );
    assert!(!bisect_retry_applies(true, pick, later, now), "an explicit --vtree is the vtree asked for");
    assert!(!bisect_retry_applies(false, Some(RETRY_VTREE_SPEC), later, now));
    assert!(
        !bisect_retry_applies(false, Some("hypergraph-bisect:imbalance=0.25"), later, now),
        "the portfolio already picked a bisection",
    );
    assert!(!bisect_retry_applies(false, pick, Some(now), now), "a compile stopped at the deadline");
}

/// `(x_1 ∨ … ∨ x_n) ∧ ⋀_i (x_i ↔ y_i)` over `x_i = i`, `y_i = n + i`: one
/// connected formula, joined by its long clause, with `2^n − 1` models.
fn paired(n: i32) -> CnfFormula {
    use crate::test_support::{clause, formula};
    let mut clauses = vec![clause(&(1..=n).collect::<Vec<_>>())];
    for i in 1..=n {
        clauses.push(clause(&[-i, n + i]));
        clauses.push(clause(&[i, -(n + i)]));
    }
    formula(2 * n as u32, clauses)
}

/// The linear vtree reads every `x` before any `y`, so the diagram keeps each
/// `x` until it meets its `y` and doubles at every level: under a byte budget
/// its compile runs out of memory. The bisection keeps each pair together and
/// compiles under the same budget. With an explicit vtree the run gives up;
/// from the default portfolio the retry answers, exactly.
#[test]
fn a_compile_out_of_memory_on_its_vtree_answers_on_the_bisection() {
    const N: i32 = 24;
    const BUDGET_BYTES: u64 = 4 << 20;
    let formula = paired(N);
    let context = Arc::new(tididi::Context::new());
    context.run(|eng| {
        let _budget = eng.limits().edit(|limits| limits.with_memory_budget_bytes(Some(BUDGET_BYTES)));
        let linear = || {
            let root = crate::vtree::build_vtree(&formula, "linear", &VtreeOptions::default()).unwrap();
            VtreeBuild { root, components: None }.bind(eng)
        };
        let explicit = Config { vtree: Some("linear".to_owned()), ..crate::test_support::minimal_config() };
        let refused = compile_with_retry(&crate::test_support::run(eng, &explicit), &formula, linear(), None);
        assert!(
            matches!(refused, Err(Error::Incomplete(_))),
            "the budget must stop the compile on the linear vtree: {:?}",
            refused.map(|(compiled, by)| (compiled.diagram.pair_count(), by)),
        );
        let config = crate::test_support::minimal_config();
        let (compiled, answered_by) =
            compile_with_retry(&crate::test_support::run(eng, &config), &formula, linear(), Some("linear"))
                .expect("the bisection compiles under the same budget");
        assert_eq!(answered_by, AnsweredBy::BisectRetry);
        assert_eq!(
            eng.model_count(&compiled.diagram).unwrap(),
            (num_bigint::BigUint::from(1u32) << N as usize) - 1u32,
        );
    });
}

/// A retry that starts too late to build its vtree gives up the way a compile
/// stopped at its deadline does; the construction's failure is not a crash.
#[test]
fn a_retry_out_of_time_for_its_vtree_gives_up() {
    let formula = paired(24);
    let context = Arc::new(tididi::Context::new());
    context.run(|eng| {
        let config = crate::test_support::minimal_config();
        let run = Run { deadline: Some(Instant::now()), ..crate::test_support::run(eng, &config) };
        let result = compile_on_bisection(&run, &formula);
        assert!(matches!(result, Err(GiveUp::Stopped)), "{:?}", result.map(|compiled| compiled.diagram.pair_count()));
    });
}
