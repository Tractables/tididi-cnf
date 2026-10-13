use super::*;
use tididi::Engine;
use crate::test_support::{self as fixtures, brute_projected_count, clause, with_clause};

/// Brute-force w(a): the number of own-show assignments of `formula`
/// feasible under each boundary assignment `a`, every other variable
/// existentially forgotten.
fn brute_boundary(formula: &CnfFormula, own: &[VarId], boundary: &[VarId]) -> Vec<BigUint> {
    (0..1usize << boundary.len())
        .map(|a| {
            let fixed = boundary.iter().enumerate().fold(formula.clone(), |f, (k, v)| {
                let d = v.0 as i32;
                with_clause(f, clause(&[if (a >> k) & 1 == 1 { d } else { -d }]))
            });
            brute_projected_count(&fixed, own.iter().map(|v| v.idx()))
        })
        .collect()
}

#[test]
fn boundary_count_refuses_expired_batch_and_recovers() {
    let eng = &Engine::new();
    let formula = fixtures::formula(2, vec![clause(&[1, 2])]);
    let vtree = Arc::new(Vtree::balanced(2));
    {
        let _deadline = eng.limits().edit(|limits| {
            limits.with_deadline(Some(std::time::Instant::now()))
        });
        assert!(component_boundary_function(
            eng, Default::default(), &formula, &vtree, &[], &[VarId(2)], &[VarId(1)],
        ).is_none());
    }
    let counts = component_boundary_function(
        eng, Default::default(), &formula, &vtree, &[], &[VarId(2)], &[VarId(1)],
    ).unwrap();
    assert_eq!(counts, vec![BigUint::from(1u32), BigUint::from(2u32)]);
}

#[test]
fn component_without_boundary_counts_once() {
    let eng = &Engine::new();
    let formula = fixtures::formula(2, vec![clause(&[1, 2])]);
    let tree = eng.bind_vtree(Vtree::balanced(2));
    let counts = component_boundary_function(
        eng, Default::default(), &formula, &tree, &[VarId(2)], &[VarId(1)], &[],
    ).unwrap();
    assert_eq!(counts, vec![BigUint::from(2u32)]);
}

#[test]
fn boundary_function_matches_bruteforce() {
    let eng = &Engine::new();
    // vars: boundary {0,1}, own-show {2,3}, hidden {4}.
    let boundary = vec![VarId(1), VarId(2)];
    let own = vec![VarId(3), VarId(4)];
    let hidden = vec![VarId(5)];
    let clauses = vec![
        clause(&[3, 1]),
        clause(&[-3, 5]),
        clause(&[4, -2, 5]),
        clause(&[1, 2]),
    ];
    let formula = fixtures::formula(5, clauses);
    let vtree = Arc::new(Vtree::balanced(5));

    let got = component_boundary_function(eng, Default::default(), &formula, &vtree, &hidden, &own, &boundary).unwrap();
    let want = brute_boundary(&formula, &own, &boundary);
    assert_eq!(got, want, "boundary function w(a) disagrees with brute force");
}

#[test]
fn decomposable_boundary_function_reads_the_supplied_whole_vtree() {
    let eng = &Engine::new();
    // Two independent clause components. Detect would compile them on local
    // trees and return a grafted root, while the dense boundary readout below
    // deliberately indexes and marginalizes against this supplied Whole root.
    // b0=false forces own s1; b0=true leaves it free. The second component
    // admits both values of own s2 after existentially forgetting h3.
    let formula = fixtures::formula(4, vec![
        clause(&[1, 2]),
        clause(&[3, 4]),
        clause(&[-3, -4]),
    ]);
    assert!(
        formula.detect_components().is_some(),
        "fixture must exercise the decomposable Whole-vs-Detect boundary",
    );
    let vtree = Arc::new(Vtree::linear_from_order(&[
        VarId(4),
        VarId(2),
        VarId(3),
        VarId(1),
    ]).unwrap());

    let got = component_boundary_function(
        eng,
        Default::default(),
        &formula,
        &vtree,
        &[VarId(4)],
        &[VarId(2), VarId(3)],
        &[VarId(1)],
    )
    .unwrap();

    assert_eq!(got, vec![BigUint::from(2u32), BigUint::from(4u32)]);
}

#[test]
fn detect_and_count_matches_bruteforce() {
    let eng = &Engine::new();
    // Separator var 0 (show, high occ). Component A over {1 own, 2 hidden},
    // component B over {3 own, 4 hidden}, both sharing only var 0. Plus an
    // S-only clause (0) to exercise the indicator-component path.
    let clauses = vec![
        // component A
        clause(&[2, 1]),
        clause(&[-2, 3]),
        clause(&[1, 3]),
        // component B
        clause(&[4, 1]),
        clause(&[-4, 5]),
        clause(&[1, 5]),
        // S-only unit clause over the separator (forces var 0 = true)
        clause(&[1]),
    ];
    let formula = fixtures::formula(5, clauses);
    let show = ShowSet::from_dimacs_ids(&[1, 2, 4]).unwrap();

    let cfg = ShatterCfg {
        max_k: 4,
        min_components: 2,
        max_comp_frac: 0.8,
        max_comp_vars: 64,
    };
    let plan = detect_shatter(&formula, &show, &cfg)
        .expect("formula should shatter on separator {0}");
    assert_eq!(plan.separator, vec![VarId(1)], "separator should be the high-occ var");
    assert!(plan.components.len() >= 2, "expected >= 2 components");

    let total =
        structured_count_with(eng, Default::default(), &plan, |f| Some(Arc::new(Vtree::balanced(f.num_vars().max(1)))))
            .unwrap();
    let want = brute_projected_count(&formula, show.iter_vars().map(|v| v.idx()));
    assert_eq!(total, want, "structured count != brute projected count");
}
