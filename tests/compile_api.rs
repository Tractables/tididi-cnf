//! `compile_cnf` options at their edges, a deadline and a component split,
//! supplied by the caller or built with the vtree; a compile cut partway; the
//! empty clause; and the errors for a vtree or split that does not fit the
//! formula.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use num_bigint::BigUint;
use num_rational::BigRational;
use num_traits::One;

use tididi::Engine;
use tididi::diagram::{Arithmetic, LiteralWeights, RationalWeights};
use tididi::test_helpers::{CnfShape, Lcg, brute_force_count, rand_cnf};
use tididi::vtree::{VarId, Vtree};
use tididi_cnf::compile::{CompileOptions, ComponentPlan, GiveUp, Output, compile_cnf};
use tididi_cnf::vtree::{Component, ComponentVtree, VtreeOptions, build_vtree, build_vtree_with_components};

use common::{brute_force_projected_count, make_formula};

/// A deadline the compile does not reach changes nothing it counts.
#[test]
fn a_deadline_ahead_leaves_the_count_alone() {
    let eng = &Engine::new();
    let deadline = Some(Instant::now() + Duration::from_secs(10));
    for (n, clauses) in [(3, vec![vec![1, 2], vec![-2, 3]]), (2, vec![])] {
        let formula = make_formula(n, clauses.clone());
        let vtree = Arc::new(Vtree::balanced(n));
        let tdd = compile_cnf(eng, &formula, &vtree, CompileOptions { deadline, ..CompileOptions::default() })
            .expect("compiles before the deadline")
            .diagram;
        assert_eq!(tdd.model_count().unwrap(), BigUint::from(brute_force_count(n, &clauses)));
    }
}

/// A deadline already passed stops the compile, of one component or of
/// several.
#[test]
fn a_passed_deadline_stops_the_compile() {
    let eng = &Engine::new();
    let deadline = Some(Instant::now() - Duration::from_secs(1));
    let vtree = Arc::new(Vtree::balanced(4));
    for (what, clauses) in [
        ("one component", vec![vec![1, 2], vec![-2, 3], vec![3, 4], vec![-1, 4]]),
        ("two components", vec![vec![1, 2], vec![3, 4]]),
        ("constant true", vec![]),
        ("constant false", vec![vec![]]),
    ] {
        let formula = make_formula(4, clauses);
        assert!(
            compile_cnf(eng, &formula, &vtree, CompileOptions { deadline, ..CompileOptions::default() }).is_err(),
            "{what}: a passed deadline must stop the compile"
        );
        let _scope = eng.limits().edit(|limits| limits.with_deadline(deadline));
        assert!(matches!(
            compile_cnf(eng, &formula, &vtree, CompileOptions::default()),
            Err(GiveUp::Stopped),
        ), "{what}: the enclosing deadline must stop the compile too");
    }
}

#[test]
fn compile_rejects_projection_variables_outside_the_formula() {
    let formula = make_formula(2, vec![vec![1, 2]]);
    let vtree = build_vtree(&formula, "portfolio", &VtreeOptions::default()).unwrap();
    for var in [VarId(0), VarId(3)] {
        for output in [Output::ProjectedCount(&[var]), Output::ProjectedFunction(&[var])] {
            let result = compile_cnf(&Engine::new(), &formula, &vtree, CompileOptions {
                output, ..CompileOptions::default()
            });
            assert!(matches!(result, Err(GiveUp::Invalid(message)) if message.contains(&var.0.to_string())));
        }
    }
}

#[test]
fn compile_rejects_missing_literal_weights_before_shortcuts_or_splitting() {
    for clauses in [vec![], vec![vec![]], vec![vec![1, 2]], vec![vec![1], vec![2]]] {
        let formula = make_formula(2, clauses);
        let vtree = build_vtree(&formula, "portfolio", &VtreeOptions::default()).unwrap();
        for plan in [ComponentPlan::Whole, ComponentPlan::Detect] {
            let result = compile_cnf(&Engine::new(), &formula, &vtree, CompileOptions {
                plan,
                output: Output::WeightedCount {
                    weights: RationalWeights::unit(1), arithmetic: Arithmetic::ExactRational,
                },
                ..CompileOptions::default()
            });
            assert!(matches!(result, Err(GiveUp::Invalid(_))));
        }
    }
}

#[test]
fn compile_rejects_invalid_splits_even_when_the_formula_is_constant() {
    for clauses in [vec![], vec![vec![]]] {
        let formula = make_formula(2, clauses);
        let vtree = build_vtree(&formula, "portfolio", &VtreeOptions::default()).unwrap();
        let split = [ComponentVtree {
            vtree: vtree.clone(),
            component: Component { clause_indices: vec![1], local_to_global: vec![VarId(1), VarId(2)] },
        }];
        let result = compile_cnf(&Engine::new(), &formula, &vtree, CompileOptions {
            plan: ComponentPlan::Split(&split), ..CompileOptions::default()
        });
        assert!(matches!(result, Err(GiveUp::Invalid(_))));

        let wrong_tree = [ComponentVtree {
            vtree: vtree.clone(),
            component: Component {
                clause_indices: (0..formula.clauses().len()).collect(),
                local_to_global: vec![VarId(1)],
            },
        }];
        let result = compile_cnf(&Engine::new(), &formula, &vtree, CompileOptions {
            plan: ComponentPlan::Split(&wrong_tree), ..CompileOptions::default()
        });
        assert!(matches!(result, Err(GiveUp::Invalid(_))));
    }
}

/// A compile cut partway gives up rather than panicking, for every output.
/// Each cut stops the compile halfway through the work a whole compile of
/// eight queens takes for that output, inside an apply or between two.
#[test]
fn a_cut_compile_gives_up_for_every_output() {
    let eng = &Engine::new();
    let (num_vars, clauses) = common::queens_cnf(8);
    let formula = make_formula(num_vars, clauses);
    let vtree = build_vtree(&formula, "portfolio", &VtreeOptions::default()).unwrap();
    // The last row's squares are hidden and the rest are shown.
    let hidden: Vec<VarId> = (57..=64).map(VarId).collect();
    let ones = vec![BigRational::one(); num_vars as usize];
    let weights = RationalWeights::from_polarities(LiteralWeights { negative: ones.clone(), positive: ones })
        .expect("both polarity columns cover every variable");
    for (name, output) in [
        ("Function", Output::Function),
        ("Count", Output::Count),
        ("WeightedCount", Output::WeightedCount { weights, arithmetic: Arithmetic::ExactRational }),
        ("ProjectedCount", Output::ProjectedCount(&hidden)),
        ("ProjectedFunction", Output::ProjectedFunction(&hidden)),
    ] {
        let options = || CompileOptions { output: output.clone(), ..CompileOptions::default() };
        let start = eng.limits().work_units();
        compile_cnf(eng, &formula, &vtree, options()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let half = (eng.limits().work_units() - start) / 2;
        let cut = common::cut_after(eng, half, || compile_cnf(eng, &formula, &vtree, options()));
        assert!(matches!(cut, Err(GiveUp::Stopped)), "{name}: a cut halfway through the work must give up");
    }
}

/// `ComponentPlan::Split` with caller-built component vtrees counts what the
/// whole formula counts.
#[test]
fn a_supplied_component_split_counts_the_whole_formula() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    // (clauses, per component: its clause indices and its variables)
    for (clauses, components) in [
        (vec![vec![1, 2], vec![3, 4]], vec![(vec![0], vec![1, 2]), (vec![1], vec![3, 4])]),
        (vec![vec![1, 2], vec![-1, 2], vec![3, 4]], vec![(vec![0, 1], vec![1, 2]), (vec![2], vec![3, 4])]),
        // An unsatisfiable component makes the whole formula unsatisfiable.
        (vec![vec![1], vec![-1], vec![3, 4]], vec![(vec![0, 1], vec![1]), (vec![2], vec![3, 4])]),
    ] {
        let split: Vec<ComponentVtree> = components
            .into_iter()
            .map(|(clause_indices, vars)| ComponentVtree {
                vtree: Arc::new(Vtree::balanced(vars.len() as u32)),
                component: Component { clause_indices, local_to_global: vars.into_iter().map(VarId).collect() },
            })
            .collect();
        let formula = make_formula(4, clauses.clone());
        let tdd = compile_cnf(eng, &formula, &vtree, CompileOptions { plan: ComponentPlan::Split(&split), ..CompileOptions::default() })
            .unwrap()
            .diagram;
        assert_eq!(tdd.model_count().unwrap(), BigUint::from(brute_force_count(4, &clauses)), "{clauses:?}");
    }
}

/// A structural vtree spec on a formula with two components builds a split
/// too, and compiling with it counts the whole formula.
#[test]
fn a_built_component_split_counts_the_whole_formula() {
    let formula = make_formula(4, vec![vec![1, 2], vec![3, 4]]);
    let built = build_vtree_with_components(&formula, "minfill-primal", &VtreeOptions::default()).unwrap();
    let split = built.components.expect("two components give a split");
    let plan = ComponentPlan::Split(&split);
    let tdd = compile_cnf(&Engine::new(), &formula, &built.root, CompileOptions { plan, ..CompileOptions::default() }).unwrap().diagram;
    assert_eq!(tdd.model_count().unwrap(), BigUint::from(9u32));
}

/// An empty clause is the constant false, and every output compiles it to
/// the false diagram, under every plan.
#[test]
fn an_empty_clause_compiles_to_false() {
    let formula = make_formula(3, vec![vec![1, 2], vec![], vec![-1, 3]]);
    let vtree = Arc::new(Vtree::balanced(3));
    let forget = [VarId(2)];
    for plan in [ComponentPlan::Detect, ComponentPlan::Whole] {
        for output in [Output::Function, Output::Count, Output::ProjectedCount(&forget), Output::ProjectedFunction(&forget)] {
            let options = CompileOptions { plan, output, ..CompileOptions::default() };
            let tdd = compile_cnf(&Engine::new(), &formula, &vtree, options).expect("false is an answer").diagram;
            assert!(tdd.is_zero());
        }
    }
}

/// A vtree that does not place exactly the formula's variables is the
/// caller's error, which the compile returns instead of panicking.
#[test]
fn a_vtree_over_other_variables_is_an_error() {
    let formula = make_formula(3, vec![vec![1, 2], vec![-2, 3]]);
    for vtree in [Vtree::balanced(2), Vtree::balanced(4), Vtree::linear_from_order(&[VarId(1), VarId(2), VarId(4)]).unwrap()] {
        let result = compile_cnf(&Engine::new(), &formula, &Arc::new(vtree), CompileOptions { plan: ComponentPlan::Whole, ..CompileOptions::default() });
        assert!(matches!(result, Err(GiveUp::Invalid(_))));
    }
}

/// A split whose components do not partition the formula — a clause in no
/// component, a variable in two, a clause naming a variable outside its
/// component, a component vtree over other variables — is the caller's
/// error, which the compile returns instead of panicking.
#[test]
fn a_split_that_does_not_partition_the_formula_is_an_error() {
    let formula = make_formula(4, vec![vec![1, 2], vec![3, 4]]);
    let vtree = Arc::new(Vtree::balanced(4));
    let pair = || Arc::new(Vtree::balanced(2));
    let component = |clause_indices: Vec<usize>, vars: [u32; 2]| ComponentVtree {
        vtree: pair(),
        component: Component { clause_indices, local_to_global: vars.map(VarId).to_vec() },
    };
    let malformed = [
        vec![component(vec![0], [1, 2])],
        vec![component(vec![0], [1, 2]), component(vec![1], [2, 3])],
        vec![component(vec![0], [1, 3]), component(vec![1], [2, 4])],
        vec![component(vec![0], [1, 2]), component(vec![1], [3, 4]), component(vec![1], [3, 4])],
        vec![
            component(vec![0], [1, 2]),
            ComponentVtree {
                vtree: Arc::new(Vtree::balanced(3)),
                component: Component { clause_indices: vec![1], local_to_global: vec![VarId(3), VarId(4)] },
            },
        ],
    ];
    for split in &malformed {
        let options = CompileOptions { plan: ComponentPlan::Split(split), ..CompileOptions::default() };
        let result = compile_cnf(&Engine::new(), &formula, &vtree, options);
        assert!(matches!(result, Err(GiveUp::Invalid(_))));
    }
}

/// `Output::ProjectedFunction`, as the projected weighted count compiles: no
/// level of the result is marginal, and it is the Boolean diagram of the
/// formula with the forgotten variables ∃-quantified, each then free and
/// doubling the count.
#[test]
fn forgetting_without_marginalizing_compiles_the_projected_function() {
    let eng = &Engine::new();
    let mut rng = Lcg::new(0xF0_4637);
    for n in 3..=6u32 {
        let vtree = Arc::new(Vtree::balanced(n));
        for _ in 0..40 {
            let clauses = rand_cnf(&mut rng, n, CnfShape { clauses: 5, width: 3 });
            let formula = make_formula(n, clauses.clone());
            for show_mask in [0b1u32, 0b101, 0b110, (1 << n) - 2] {
                let show: Vec<u32> = (0..n).filter(|&i| (show_mask >> i) & 1 == 1).collect();
                let hidden: Vec<VarId> = (0..n).filter(|i| !show.contains(i)).map(|i| VarId(i + 1)).collect();
                let opts = CompileOptions {
                    output: Output::ProjectedFunction(&hidden),
                    plan: ComponentPlan::Whole,
                    ..CompileOptions::default()
                };
                let diagram = compile_cnf(eng, &formula, &vtree, opts).unwrap().diagram;
                let label = format!("clauses {clauses:?}, show {show:?}");
                assert!(
                    vtree.bottomup().all(|t| !diagram.level(t).is_marginal()),
                    "a level was marginalized, {label}"
                );
                assert_eq!(
                    diagram.model_count().unwrap(),
                    brute_force_projected_count(&formula, &show) << hidden.len(),
                    "{label}"
                );
            }
        }
    }
}

/// An apply refused under the batch's memory budget gives up with the limit
/// it reached, in the words a benchmark log is read for, and prints nothing
/// itself: the caller says it.
#[test]
fn a_refused_apply_gives_up_naming_its_limit() {
    let eng = &Engine::new();
    let formula = make_formula(3, vec![vec![1, 2], vec![-2, 3]]);
    let vtree = Arc::new(Vtree::balanced(3));
    let _budget = eng.limits().edit(|limits| limits.with_memory_budget_bytes(Some(0)));
    let give_up = compile_cnf(eng, &formula, &vtree, CompileOptions::default()).err().expect("no byte is budgeted");
    assert_eq!(give_up, GiveUp::Refused { limit: tididi::OperationError::OverBudget, refused_bytes: None });
    assert_eq!(give_up.to_string(), "apply reached a limit with no recorded allocator refusal (cap=OverBudget)");
}
