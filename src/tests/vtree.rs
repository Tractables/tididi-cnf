use super::*;
use crate::test_support::{dimacs, formula, three_variable_formula};

/// A spec is vitri's grammar, whose own tests cover it: a spec it reads is
/// accepted, and one it does not is a vtree-spec error.
#[test]
fn a_spec_is_vitri_s_grammar() {
    assert!(validate_vtree_spec("minfill-primal").is_ok());
    assert!(matches!(validate_vtree_spec("nonexistent"), Err(Error::VtreeSpec(_))));
}

/// A spec's family is its text before the first colon, whatever its
/// parameters name.
#[test]
fn a_spec_family_is_the_text_before_its_parameters() {
    assert_eq!(spec_family("hypergraph-bisect:imbalance=0.40"), "hypergraph-bisect");
    assert_eq!(spec_family("flowcutter-primal:binarize=hypergraph-bisect"), "flowcutter-primal");
    assert_eq!(spec_family("linear"), "linear");
}

/// A formula with no variables has no vtree leaves; the build says so with an
/// error instead of panicking.
#[test]
fn a_formula_without_variables_is_an_error_not_a_panic() {
    let empty = formula(0, Vec::new());
    let options = VtreeOptions::default();
    assert!(matches!(build_vtree(&empty, "linear", &options), Err(Error::VtreeSpec(_))));
    assert!(matches!(build_vtree_with_components(&empty, "portfolio", &options), Err(Error::VtreeSpec(_))));
}

/// A build bound to a batch's engine puts its root and component trees in
/// the batch's context, so compiles on them run under the batch's limits and
/// memory hooks, and hooks set inside the batch end with it.
#[test]
fn component_trees_retain_run_context_and_share_batch_limits() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tididi::limits::{LimitConfig, MemoryHooks};

    let context = Arc::new(tididi::Context::new());
    let allocations = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&allocations);
    let hooks = MemoryHooks::new(
        move |_| { observed.fetch_add(1, Ordering::Relaxed); },
        || 0,
        || None,
        || {},
    );
    let output = context.run(|engine| {
        let _hooks = engine.limits().edit(|config| config.with_memory_hooks(hooks));
        let built = VtreeBuild {
            root: Arc::new(Vtree::balanced(6)),
            components: Some(vec![
                ComponentVtree {
                    vtree: Arc::new(Vtree::balanced(2)),
                    component: Component { clause_indices: vec![0], local_to_global: vec![VarId(1), VarId(2)], implied: Vec::new() },
                },
                ComponentVtree {
                    vtree: Arc::new(Vtree::balanced(4)),
                    component: Component {
                        clause_indices: vec![1],
                        local_to_global: vec![VarId(3), VarId(4), VarId(5), VarId(6)],
                        implied: Vec::new(),
                    },
                },
            ]),
            implied: Vec::new(),
        }
        .bind(engine);
        assert!(Arc::ptr_eq(built.root.context(), &context));
        for component in built.components.as_ref().unwrap() {
            assert!(Arc::ptr_eq(component.vtree.context(), &context));
            let before = allocations.load(Ordering::Relaxed);
            let diagram = engine.clause(&component.vtree, [1, 2]).unwrap();
            let expected = num_bigint::BigUint::from(3u32)
                << (component.vtree.num_vars() - 2) as usize;
            assert_eq!(engine.model_count(&diagram).unwrap(), expected);
            assert!(allocations.load(Ordering::Relaxed) > before);
        }
        {
            let _budget = engine.limits().scope(
                LimitConfig::none().with_memory_budget_bytes(Some(0)),
            );
            assert_eq!(
                engine.clause(&built.root, [1, 2]).unwrap_err(),
                tididi::OperationError::OverBudget,
            );
        }
        engine.clause(&built.root, [1, 2]).unwrap()
    });
    assert!(Arc::ptr_eq(output.vtree().context(), &context));
    assert_eq!(output.model_count().unwrap(), num_bigint::BigUint::from(48u32));
    let before = allocations.load(Ordering::Relaxed);
    context.run(|engine| {
        let tree = engine.bind_vtree(Vtree::balanced(64));
        engine.clause(&tree, [1, 2]).unwrap();
    });
    assert_eq!(allocations.load(Ordering::Relaxed), before);
}

/// vitri's linear vtree arrives as the library's linear vtree over the same
/// variables.
#[test]
fn a_linear_vitri_vtree_converts_its_root_shape_and_variable_ids() {
    let root = build_vtree(
        &three_variable_formula(),
        "linear",
        &VtreeOptions::default(),
    )
    .expect("the bridge must build a linear vtree");

    assert_eq!(root.to_text(), Vtree::linear(3).to_text());
}

/// A split build keeps each component's clauses and its map to the formula's
/// variables, and its root keeps a variable in no clause.
#[test]
fn vitri_build_preserves_component_clause_and_variable_mappings() {
    // (x1 ∨ x2) ∧ (¬x3 ∨ x4): two disconnected, two-variable components.
    // Variable 5 appears in no clause and must remain a free leaf in
    // the whole-formula graft rather than disappearing with the split.
    let formula = dimacs(5, &[&[1, 2], &[-3, 4]]);
    let built = build_vtree_with_components(
        &formula,
        "minfill-primal",
        &VtreeOptions::default(),
    )
    .expect("the bridge must build each disconnected component");
    let components = built
        .components
        .expect("split construction keeps its components");

    assert_eq!(components.len(), 2);
    assert_eq!(
        components[0].component,
        Component { clause_indices: vec![0], local_to_global: vec![VarId(1), VarId(2)], implied: Vec::new() },
    );
    assert_eq!(
        components[1].component,
        Component { clause_indices: vec![1], local_to_global: vec![VarId(3), VarId(4)], implied: Vec::new() },
    );
    for component in components {
        let mut local_vars: Vec<_> = component
            .vtree
            .leaf_bottomup()
            .map(|(_, var)| var.0)
            .collect();
        local_vars.sort_unstable();
        assert_eq!(local_vars, [1, 2]);
    }
    let mut root_vars: Vec<_> = built.root.leaf_bottomup().map(|(_, var)| var.0).collect();
    root_vars.sort_unstable();
    assert_eq!(root_vars, [1, 2, 3, 4, 5]);
}

/// The default options select vtrees by vitri's clause-balance objective,
/// and `by_peak_width` by its peak-width one.
#[test]
fn selection_objectives_map_to_vitri_objectives() {
    let plain = vitri_selection_ctx(&VtreeOptions::default());
    assert!(matches!(
        plain.objective,
        vitri::decompose::SelectionObjective::ClauseBalance
    ));

    let peak = vitri_selection_ctx(&VtreeOptions::by_peak_width());
    assert!(matches!(
        peak.objective,
        vitri::decompose::SelectionObjective::PeakWidthAll
    ));
}

/// The run's measured structure profile reaches vitri's selection, and a run
/// without one passes none.
#[test]
fn source_structure_profile_propagates_through_the_selection_context() {
    let profile = vitri::score::StructureProfile::from_coefficients(0.2052, 0.2148);

    let default_config = crate::test_support::minimal_config();
    let default_ctx = VtreeOptions::default().with_run_policy(&default_config);
    assert_eq!(default_ctx.source_structure_profile, None);
    assert_eq!(
        vitri_selection_ctx(&default_ctx).source_profile,
        None
    );

    let config = crate::config::Config {
        source_structure_profile: Some(profile),
        ..crate::test_support::minimal_config()
    };
    let ctx = VtreeOptions::default().with_run_policy(&config);
    assert_eq!(ctx.source_structure_profile, Some(profile));
    assert_eq!(
        vitri_selection_ctx(&ctx).source_profile,
        Some(profile),
        "the exact measured source profile must reach vitri's selection",
    );
}

/// A build under a deadline gives vitri's construction all the time left
/// before it.
#[test]
fn vtree_build_config_uses_the_callers_whole_remaining_window() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let config = vitri_vtree_build_config(
        "linear",
        Some(deadline),
        vitri::config::ComponentPolicy::Whole,
        None,
    );

    assert_eq!(
        config.construction_budget,
        vitri::config::ConstructionBudget::WholeRemaining
    );
    assert_eq!(
        config.construction_deadline(std::time::Instant::now()),
        Some(deadline)
    );
}

/// A build that did not split hands its implied clauses to the compile after
/// the formula's own, and the count stays the formula's; with none it hands
/// the formula over as it is.
#[test]
fn a_whole_build_conjoins_its_implied_clauses_and_keeps_the_count() {
    use crate::compile::{CompileOptions, ComponentPlan, compile_cnf};
    // (x1 ∨ x2) ∧ (¬x2 ∨ x3) implies x1 ∨ x3.
    let formula = dimacs(3, &[[1, 2], [-2, 3]]);
    let lit = |var: usize| vitri::cnf::Literal::new(vitri::cnf::VarId::from_idx(var - 1), true);
    let implied = vitri::cnf::Clause { literals: vec![lit(1), lit(3)] };
    let built = VtreeBuild { root: Arc::new(Vtree::balanced(3)), components: None, implied: vec![implied.clone()] };

    let conjoined = built.conjoined(&formula);
    assert_eq!(conjoined.clauses().len(), 3);
    assert_eq!(conjoined.clauses()[2], implied);
    let none = VtreeBuild { implied: Vec::new(), ..built.clone() };
    assert!(matches!(none.conjoined(&formula), Cow::Borrowed(_)));

    let eng = tididi::Engine::new();
    let count = |f: &CnfFormula| {
        let options = CompileOptions { plan: ComponentPlan::Whole, ..CompileOptions::default() };
        eng.model_count(&compile_cnf(&eng, f, &built.root, options).unwrap().diagram).unwrap()
    };
    assert_eq!(count(&conjoined), count(&formula));
}

/// A stage that asks for implied clauses asks vitri with the run's budget;
/// one that does not runs no pass.
#[test]
fn a_build_asked_for_implied_clauses_carries_the_runs_budget() {
    let base = crate::test_support::minimal_config();
    let config = Config { vitri: vitri::RunConfig { implicates_ms: Some(500), ..base.vitri.clone() }, ..base };
    let asked = VtreeOptions::default().with_implied_clauses(&config);
    let request = vitri_vtree_build_config("portfolio", None, vitri::config::ComponentPolicy::Split, asked.implicates_ms);
    assert_eq!(request.implicates_ms, Some(500));
    assert_eq!(VtreeOptions::default().with_run_policy(&config).implicates_ms, None);
}
