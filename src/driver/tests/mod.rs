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
