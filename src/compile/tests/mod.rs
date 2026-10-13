use super::*;
use tididi::Engine;
use tididi::limits::StopAt;
use crate::test_support::clause;
use std::time::Duration;
use tididi::diagram::RationalWeights;
use num_bigint::{BigInt, BigUint};
use num_rational::BigRational;

/// Connected 3-CNF: every variable shares a clause with another, so the
/// clauses land at internal vtree nodes.
const CLAUSES: &[&[i32]] = &[
    &[1, 2, 3],
    &[-2, 4, 5],
    &[3, -5, 6],
    &[-1, 6, 7],
    &[4, -7, 8],
    &[-3, 5, -8],
    &[2, -6, 8],
    &[-4, 7, 1],
];

fn subject() -> (CnfFormula, Vec<Component>) {
    let formula = crate::test_support::formula(8, CLAUSES.iter().map(|c| clause(c)).collect());
    let components = vec![Component {
        clause_indices: (0..formula.clauses().len()).collect(),
        local_to_global: (1..=8).map(VarId).collect(),
    }];
    (formula, components)
}

fn portfolio(sub: &CnfFormula) -> Arc<Vtree> {
    crate::vtree::build_vtree(sub, "portfolio", &crate::vtree::VtreeOptions::default()).unwrap()
}

/// The streaming loop must install the wallclock guard itself, for every
/// caller, so a caller that reaches it directly passes through no wrapper
/// and still gets one: by the time the loop asks for a component's vtree,
/// the caller's deadline is the apply-level cut armed in the engine's
/// limits, and the loop restores the previous limits when it returns.
#[test]
fn streaming_loop_installs_the_wall_guard_for_a_direct_caller() {
    let eng = &Engine::new();
    let (formula, components) = subject();
    let deadline = Instant::now() + Duration::from_secs(3600);
    let mut armed = Vec::new();
    let build = |sub: &CnfFormula| {
        armed.push(eng.limits().armed().stop_rules().unconditional);
        Some(portfolio(sub))
    };
    let counted = model_count_streaming(eng, &formula, &components, build, None, Some(deadline), CompileRun::default());
    assert!(counted.is_some(), "the component compiles well inside its deadline");
    assert_eq!(
        armed,
        [Some(StopAt::Time(deadline))],
        "the streaming loop did not arm its deadline as the apply-level cut — a caller \
         that reaches it directly has no wall enforcement inside an apply",
    );
    assert_eq!(eng.limits().armed().stop_rules().unconditional, None, "the guard outlived the loop");
}

#[test]
fn streaming_cannot_extend_or_clear_the_enclosing_deadline() {
    let eng = &Engine::new();
    let (formula, components) = subject();
    let outer = Instant::now() - Duration::from_secs(1);
    let _scope = eng.limits().edit(|s| s.with_deadline(Some(outer)));
    for child in [None, Some(Instant::now() + Duration::from_secs(3600))] {
        let mut observed = Vec::new();
        let build = |sub: &CnfFormula| {
            observed.push(eng.limits().armed().stop_rules().unconditional);
            Some(portfolio(sub))
        };
        let result = model_count_streaming(eng, &formula, &components, build, None, child, CompileRun::default());
        assert!(result.is_none(), "a nested compile escaped an expired enclosing deadline");
        assert!(observed.iter().all(|stop| *stop == Some(StopAt::Time(outer))));
        assert_eq!(eng.limits().armed().stop_rules().unconditional, Some(StopAt::Time(outer)));
    }
}

fn decomposable_formula_and_split() -> (CnfFormula, Arc<Vtree>, Vec<ComponentVtree>) {
    let formula = crate::test_support::formula(4, vec![
        crate::test_support::clause(&[1, 2]),
        crate::test_support::clause(&[3, 4]),
    ]);
    let root = Arc::new(Vtree::linear_from_order(&[
        VarId(4),
        VarId(2),
        VarId(3),
        VarId(1),
    ]).unwrap());
    let components = vec![
        ComponentVtree {
            vtree: Arc::new(Vtree::linear_from_order(&[VarId(2), VarId(1)]).unwrap()),
            component: Component { clause_indices: vec![0], local_to_global: vec![VarId(1), VarId(2)] },
        },
        ComponentVtree {
            vtree: Arc::new(Vtree::linear_from_order(&[VarId(1), VarId(2)]).unwrap()),
            component: Component { clause_indices: vec![1], local_to_global: vec![VarId(3), VarId(4)] },
        },
    ];
    (formula, root, components)
}

#[test]
fn whole_plan_keeps_the_supplied_root_on_a_decomposable_formula() {
    let eng = &Engine::new();
    let (formula, supplied, _) = decomposable_formula_and_split();

    let opts = CompileOptions {
        plan: ComponentPlan::Whole,
        ..CompileOptions::default()
    };
    let tdd = compile_cnf(eng, &formula, &supplied, opts).unwrap().diagram;

    assert_eq!(tdd.model_count().unwrap(), BigUint::from(9u32));
    assert!(
        Arc::ptr_eq(tdd.vtree(), &supplied),
        "Whole must compile on the caller's exact root instead of detecting and grafting components",
    );
}

#[test]
fn whole_mc_plan_keeps_the_supplied_order_and_exact_count() {
    let eng = &Engine::new();
    let (formula, supplied, _) = decomposable_formula_and_split();
    let supplied_order: Vec<_> = supplied.leaf_bottomup().map(|(_, var)| var).collect();

    let opts = CompileOptions {
        output: Output::Count,
        plan: ComponentPlan::Whole,
        ..CompileOptions::default()
    };
    let tdd = compile_cnf(eng, &formula, &supplied, opts).unwrap().diagram;

    assert_eq!(tdd.model_count().unwrap(), BigUint::from(9u32));
    assert_eq!(
        tdd.vtree()
            .leaf_bottomup()
            .map(|(_, var)| var)
            .collect::<Vec<_>>(),
        supplied_order,
        "Whole MC must preserve the caller's order instead of detecting and grafting components",
    );
}

#[test]
fn split_plan_preserves_projected_count_across_components() {
    let eng = &Engine::new();
    let (formula, root, components) = decomposable_formula_and_split();
    let projected = [VarId(2), VarId(4)];

    let opts = CompileOptions {
        output: Output::ProjectedCount(&projected),
        plan: ComponentPlan::Split(&components),
        ..CompileOptions::default()
    };
    let compiled = compile_cnf(eng, &formula, &root, opts).expect("split projected compile");
    let count = compiled.diagram.model_count().unwrap() >> compiled.forgotten;

    assert_eq!(count, BigUint::from(4u32));
}

#[test]
fn split_plan_preserves_weighted_projected_count_across_components() {
    let eng = &Engine::new();
    let (formula, root, components) = decomposable_formula_and_split();
    let projected = [VarId(2), VarId(4)];

    let opts = CompileOptions {
        output: Output::ProjectedFunction(&projected),
        plan: ComponentPlan::Split(&components),
        ..CompileOptions::default()
    };
    let tdd = compile_cnf(eng, &formula, &root, opts).expect("split weighted-projected compile").diagram;
    let integer = |value: i64| BigRational::from_integer(BigInt::from(value));
    let weights = RationalWeights::from_polarities(tididi::diagram::LiteralWeights {
        positive: vec![integer(2), integer(1), integer(3), integer(1)],
        negative: vec![integer(5), integer(1), integer(7), integer(1)],
    })
    .expect("both polarity vectors have one entry per variable");
    let value = tdd.evaluate(&weights).unwrap() / integer(4);

    assert_eq!(value, integer(70));
}

/// A projected variable in no clause is free in the diagram and doubles its
/// count, so [`Compiled::forgotten`] counts it on every route the compile
/// takes — the whole pass, a split, and the clause-free shortcut — and the
/// diagram's count divided by `2^forgotten` is the projected count, integer
/// and weighted alike.
#[test]
fn a_free_projected_variable_counts_as_forgotten_on_every_route() {
    let eng = &Engine::new();
    let integer = |value: i64| BigRational::from_integer(BigInt::from(value));
    // (x1 ∨ x2) ∧ (x3 ∨ x4) over five variables, x2 and x5 projected: x5 is
    // in no clause. Projected variables weigh 1.
    let split = crate::test_support::formula(5, vec![clause(&[1, 2]), clause(&[3, 4])]);
    let clause_free = crate::test_support::formula(3, vec![]);
    let routes = [
        ("whole", &split, ComponentPlan::Whole, vec![VarId(2), VarId(5)]),
        ("split", &split, ComponentPlan::Detect, vec![VarId(2), VarId(5)]),
        ("clause-free", &clause_free, ComponentPlan::Whole, vec![VarId(2)]),
    ];
    for (route, formula, plan, projected) in routes {
        let n = formula.num_vars() as usize;
        let show: Vec<usize> = (0..n).filter(|&v| !projected.contains(&VarId(v as u32 + 1))).collect();
        let pairs: Vec<(BigRational, BigRational)> = (0..n)
            .map(|v| if show.contains(&v) { (integer(3 + v as i64), integer(2)) } else { (integer(1), integer(1)) })
            .collect();
        let vtree = crate::vtree::build_vtree(formula, "minfill-primal", &crate::vtree::VtreeOptions::default()).unwrap();
        let compile = |output| {
            compile_cnf(eng, formula, &vtree, CompileOptions { plan, output, ..CompileOptions::default() })
                .unwrap_or_else(|_| panic!("{route}: the compile gave up"))
        };

        let counted = compile(Output::ProjectedCount(&projected));
        assert_eq!(
            counted.diagram.model_count().unwrap() >> counted.forgotten,
            crate::test_support::brute_projected_count(formula, show.iter().copied()),
            "{route}: projected count",
        );

        let function = compile(Output::ProjectedFunction(&projected));
        let weights = RationalWeights::from_polarities(tididi::diagram::LiteralWeights {
            positive: pairs.iter().map(|(_, positive)| positive.clone()).collect(),
            negative: pairs.iter().map(|(negative, _)| negative.clone()).collect(),
        })
        .expect("both polarity vectors have one entry per variable");
        assert_eq!(
            function.diagram.evaluate(&weights).unwrap() / integer(1i64 << function.forgotten),
            crate::test_support::brute_projected_weighted_count(formula, show.iter().copied(), &pairs),
            "{route}: projected weighted count",
        );
    }
}

/// The compile returns the largest diagram its steps reached, which is never
/// smaller than the diagram it returns, and the streaming count reports the
/// same peak for the same pass.
#[test]
fn the_compile_returns_its_peak_beside_the_diagram() {
    let eng = &Engine::new();
    let (formula, components) = subject();
    let vtree = portfolio(&formula);
    let options = CompileOptions { output: Output::Count, plan: ComponentPlan::Whole, ..CompileOptions::default() };
    let compiled = compile_cnf(eng, &formula, &vtree, options).unwrap();
    assert!(compiled.peak_pairs >= compiled.diagram.pair_count().max(1));

    let (_, streamed_peak) =
        model_count_streaming(eng, &formula, &components, |_| Some(Arc::clone(&vtree)), None, None, CompileRun::default()).unwrap();
    assert_eq!(streamed_peak, compiled.peak_pairs);
}

/// A formula over `num_vars` variables from DIMACS clauses.
fn dimacs(num_vars: u32, clauses: &[Vec<i32>]) -> CnfFormula {
    crate::test_support::formula(num_vars, clauses.iter().map(|c| clause(c)).collect())
}

/// The streaming count of `formula` over `components` on balanced component
/// vtrees, with every variable outside `show` (zero-based) forgotten.
fn streaming_projected(formula: &CnfFormula, show: &[usize], components: &[Component]) -> BigUint {
    let forget: Vec<VarId> =
        (0..formula.num_vars()).filter(|&i| !show.contains(&(i as usize))).map(|i| VarId(i + 1)).collect();
    let build = |f: &CnfFormula| Some(Arc::new(Vtree::balanced(f.num_vars())));
    model_count_streaming(&Engine::new(), formula, components, build, Some(&forget), None, CompileRun::default())
        .expect("no deadline is armed")
        .0
}

/// The streaming count against brute force, on formulas that split into
/// several components, one of them unsatisfiable, with variables in no
/// clause, and with no clause at all. One engine serves every count, so each
/// starts on the scratch the previous formula left behind.
#[test]
fn streaming_counts_match_brute_force() {
    let eng = &Engine::new();
    let balanced = |f: &CnfFormula| Some(Arc::new(Vtree::balanced(f.num_vars())));
    // (variables, clauses, components)
    for (num_vars, clauses, components) in [
        (8, vec![vec![1, 2], vec![-1, -2], vec![3, 4], vec![4, 5], vec![6, 7], vec![-7, 8]], 3),
        // The middle component is unsatisfiable.
        (4, vec![vec![1, 2], vec![-1, -2], vec![3], vec![-3], vec![4]], 3),
        // Variables 5 and 6 are in no clause, so in no component.
        (6, vec![vec![1, 2], vec![-1, -2], vec![3, 4], vec![-3, -4]], 2),
        (6, vec![vec![1, 2], vec![-1, -2], vec![3, 4], vec![4, 5]], 2),
        (5, vec![vec![1, 2, 3], vec![-1, -3], vec![4, 5], vec![-4, 5]], 2),
        (5, vec![], 0),
    ] {
        let formula = dimacs(num_vars, &clauses);
        let split = crate::vtree::split_components(&formula, "minfill-primal");
        assert_eq!(split.len(), components, "{clauses:?}");
        let (count, _) = model_count_streaming(eng, &formula, &split, balanced, None, None, CompileRun::default()).unwrap();
        let expected = crate::test_support::brute_projected_count(&formula, 0..num_vars as usize);
        assert_eq!(count, expected, "{clauses:?}");
    }
}

/// The streaming count given a forget set, on one whole-formula component:
/// every variable is in some clause, so its product is the whole projected
/// count, with `2^k` divided out per component.
#[test]
fn streaming_projected_counts_match_brute_force() {
    for (num_vars, clauses, show) in [
        // (x ∨ y) ∧ (¬y ∨ z), project y: 3.
        (3, vec![vec![1, 2], vec![-2, 3]], vec![0, 2]),
        // x ↔ y, project y: 2. y is determined by x, but projecting it must
        // keep both x values.
        (2, vec![vec![-1, 2], vec![1, -2]], vec![0]),
        (4, vec![vec![1, 2], vec![3, 4], vec![-1, 3], vec![-4, 2]], vec![0, 2]),
    ] {
        let formula = dimacs(num_vars, &clauses);
        let component = Component {
            clause_indices: (0..formula.clauses().len()).collect(),
            local_to_global: (1..=num_vars).map(VarId).collect(),
        };
        assert_eq!(
            streaming_projected(&formula, &show, std::slice::from_ref(&component)),
            crate::test_support::brute_projected_count(&formula, show.iter().copied()),
            "{clauses:?}, show {show:?}",
        );
    }
}

/// The streaming count divides each component by its own number of
/// forgotten variables, not by the running total over the components so far.
/// Three disjoint components with one projected variable each drive that
/// total to 1, 2, 3, enough to shift a component's count to 0 and report the
/// formula unsatisfiable. The components come from `split_components` on
/// a structural vtree kind, the only kind for which it splits components.
#[test]
fn streaming_projected_divisor_is_per_component() {
    let formula = dimacs(6, &[vec![1, 2], vec![3, 4], vec![5, 6]]);
    let show = [0, 2, 4];
    let components = crate::vtree::split_components(&formula, "portfolio");
    assert!(components.len() >= 3, "the fixture must split into three components, got {}", components.len());
    assert_eq!(crate::test_support::brute_projected_count(&formula, show), BigUint::from(8u32));
    assert_eq!(streaming_projected(&formula, &show, &components), BigUint::from(8u32));
}

/// A projected variable in no clause contributes a factor one to the
/// streaming count, both with no clauses at all and beside a component.
#[test]
fn streaming_projected_free_and_empty_vars() {
    for (num_vars, clauses) in [(2, vec![]), (3, vec![vec![1, 2]])] {
        let formula = dimacs(num_vars, &clauses);
        let components = crate::vtree::split_components(&formula, "balanced");
        assert_eq!(
            streaming_projected(&formula, &[0], &components),
            crate::test_support::brute_projected_count(&formula, [0]),
            "{clauses:?}, {num_vars} variables",
        );
    }
}

/// A compile that stops or is refused ends the run without a count, and the
/// error says so: a library caller has no stderr log to read the reason from.
#[test]
fn a_give_up_reads_as_a_run_without_an_answer() {
    let refused = GiveUp::Refused { limit: OperationError::OverBudget, refused_bytes: None };
    for give_up in [GiveUp::Stopped, refused] {
        let error = Error::from(give_up);
        assert!(matches!(&error, Error::Incomplete(message) if message.starts_with("no answer: ")), "{error:?}");
    }
}
