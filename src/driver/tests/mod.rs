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
