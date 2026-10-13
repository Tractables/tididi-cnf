//! Projected counting in this crate's compile against brute force:
//! projection during compile (`Output::ProjectedCount`), under every
//! component plan. Variable indices in `show` sets are zero-based; clauses
//! are DIMACS.

mod common;
use tididi::Engine;
use std::sync::Arc;

use num_bigint::BigUint;

use tididi::test_helpers::{CnfShape, Lcg, rand_cnf};
use tididi_bonsai::compile::{CompileOptions, Output, compile_cnf};
use tididi::vtree::{VarId, Vtree};
use vitri::cnf::CnfFormula;

/// The variables with the given zero-based indices.
fn vars(indices: impl IntoIterator<Item = u32>) -> Vec<VarId> {
    indices.into_iter().map(|i| VarId(i + 1)).collect()
}

/// The variables of `0..n` outside `show`.
fn hidden(n: u32, show: &[u32]) -> Vec<VarId> {
    vars((0..n).filter(|i| !show.contains(i)))
}

/// `Output::ProjectedCount`: the count halved once per
/// variable the compile reports forgotten is the projected count.
#[test]
fn compile_project_counts() {
    // (clauses, variables, projected index, projected count)
    for (clauses, n, projected, expected) in [
        // (x ∨ y) ∧ (¬y ∨ z), project y: 3 of the (x, z) pairs extend.
        (vec![vec![1, 2], vec![-2, 3]], 3, 1, 3u32),
        // (a ∨ b) ∧ (x ∨ w) ∧ (a ∨ x), project x. The clause (a ∨ x) spans
        // both root subtrees, so x is forgotten at the root after the left
        // subtree has gone marginal; negation must treat that marginal level
        // as alive rather than index its (absent) nodes.
        (vec![vec![1, 2], vec![3, 4], vec![1, 3]], 4, 2, 6),
    ] {
        let formula = common::make_formula(n, clauses);
        let project = vars([projected]);
        let compiled = compile_cnf(
            &Engine::new(),
            &formula,
            &Arc::new(Vtree::balanced(n)),
            CompileOptions { output: Output::ProjectedCount(&project), ..CompileOptions::default() },
        )
        .unwrap();
        let k = compiled.forgotten;
        let raw = compiled.diagram.model_count().unwrap();
        assert_eq!(raw.clone() >> k, BigUint::from(expected), "raw {raw}, k {k}");
    }
}

/// `Output::ProjectedCount` on random CNFs of up to seven variables against brute
/// force. The `>> |projected|` divisor needs every projected variable left
/// in the vtree to be free, so, as in the driver, hidden-variable
/// elimination runs first and turns a unit-pinned projected variable into a
/// free leaf. Projected counting is invariant under it, so the oracle runs
/// on the original clauses.
#[test]
fn compile_project_matches_brute_force() {
    let eng = &Engine::new();
    let mut rng = Lcg::new(0x9E0EC7);
    for n in 3..=7u32 {
        let vtree = Arc::new(Vtree::balanced(n));
        for _ in 0..150 {
            let clauses = rand_cnf(&mut rng, n, CnfShape { clauses: 5, width: n as usize });
            let formula = common::make_formula(n, clauses.clone());

            for show_mask in [0u32, 1, 2, 0b101, (1u32 << n) - 1, 0b1010] {
                let show_mask = show_mask & ((1u32 << n) - 1);
                let show: Vec<u32> = (0..n).filter(|&i| (show_mask >> i) & 1 == 1).collect();
                let projected = hidden(n, &show);
                if projected.is_empty() {
                    continue;
                }
                let expected = common::brute_force_projected_count(&formula, &show);
                let show_set = vitri::cnf::ShowSet::<vitri::cnf::Reduced>::from_vars(
                    show.iter().map(|&var| vitri::cnf::VarId::from_idx(var as usize)),
                );
                let reduced = vitri::projection::eliminate_hidden(&formula, &show_set)
                    .expect("the show set is inside the formula");
                // Elimination can derive the empty clause, which does not compile.
                let got = if reduced.clauses().iter().any(|c| c.literals.is_empty()) {
                    BigUint::from(0u32)
                } else {
                    let tdd = compile_cnf(
                        eng,
                        &reduced,
                        &vtree,
                        CompileOptions { output: Output::ProjectedCount(&projected), ..CompileOptions::default() },
                    )
                    .unwrap()
                    .diagram;
                    tdd.model_count().unwrap() >> projected.len()
                };
                assert_eq!(got, expected, "clauses {clauses:?}, n {n}, show {show:?}");
            }
        }
    }
}

/// A variable in no clause counts the same under every component plan, with
/// the hidden variables forgotten and the count halved once per variable the
/// compile reports forgotten. A projected one
/// is a factor one: the whole compile forgets it at the root, a split grafts
/// it in free beside the components, where it still counts ×2 in the raw
/// count, and a formula with no clause at all compiles to the constant one,
/// where every variable does. A shown one stays a factor two. The split plan's
/// trees come from Vitri's portfolio build; the whole plan compiles on a
/// balanced root.
#[test]
fn every_plan_counts_a_free_variable_once() {
    use tididi_bonsai::vtree::ComponentVtree;
    use tididi_bonsai::compile::ComponentPlan;
    let eng = &Engine::new();
    let check = |formula: &CnfFormula, clauses: &[Vec<i32>], show: &[u32], components: &[ComponentVtree]| {
        let n = formula.num_vars();
        let root = Arc::new(Vtree::balanced(n));
        let expected = common::brute_force_projected_count(formula, show);
        for (name, plan) in [
            ("whole", ComponentPlan::Whole),
            ("split", ComponentPlan::Split(components)),
            ("detect", ComponentPlan::Detect),
        ] {
            let forget = hidden(n, show);
            let opts = CompileOptions { output: Output::ProjectedCount(&forget), plan, ..CompileOptions::default() };
            let compiled = compile_cnf(eng, formula, &root, opts)
                .expect("the compile completes");
            let count = compiled.diagram.model_count().unwrap() >> compiled.forgotten;
            assert_eq!(count, expected, "{name} plan, {n} variables, {} clauses, show {show:?}", clauses.len());
        }
    };
    // Two components, (x1 ∨ x2) and (x3 ∨ x4); x5 and x6 are in no clause.
    let clauses = vec![vec![1, 2], vec![3, 4]];
    // (variables, show): x5 projected; x5 and x6 projected; x5 shown.
    for (n, show) in [(5, vec![0, 2]), (6, vec![0, 2]), (5, vec![0, 2, 4])] {
        let formula = common::make_formula(n, clauses.clone());
        let components = tididi_bonsai::vtree::build_vtree_with_components(
            &formula,
            "portfolio",
            &tididi_bonsai::vtree::VtreeOptions::by_peak_width(),
        )
        .unwrap()
        .components
        .expect("the fixture splits into components");
        assert_eq!(components.len(), 2, "one component per clause");
        check(&formula, &clauses, &show, &components);
    }
    // No clause at all, x1 shown and x2 and x3 projected: no components to split.
    check(&common::make_formula(3, vec![]), &[], &[0], &[]);
}
