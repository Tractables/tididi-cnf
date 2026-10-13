//! The marginalizing compile (`Output::Count`, the `--mc`
//! path). Every formula of the shared case table, plus a few with separated
//! components and small counts, is compiled on every vtree shape and must
//! count right and pass the library's marginal checks; and a set of saved
//! formulas, toys that once counted wrong and competition instances, must
//! count right.

mod common;

use tididi::Engine;

use num_bigint::BigUint;

use tididi::test_helpers::check::marginal::{
    check_inline_discipline, check_marginal_canonical_form, check_no_orphan_slots,
    check_slot_count_uniqueness, subsumed_marginal_data_violations,
};
use tididi_bonsai::vtree::VtreeOptions;
use tididi_bonsai::compile::{CompileOptions, ComponentPlan, Output, compile_cnf};
use tididi_bonsai::vtree::build_vtree;

use tididi::test_helpers::brute_force_count;

use common::{all_vtree_shapes, get_test_cases, load_cnf, make_formula};

#[test]
fn marginalizing_compile_is_canonical_on_every_shape() {
    let eng = &Engine::new();
    let mut cases = get_test_cases();
    cases.extend([
        // Two components, one side marginalized under its parent, then that
        // parent under the root.
        (5, vec![vec![1, 2, 3], vec![-1, -2], vec![-2, -3], vec![4, 5], vec![-4, -5]]),
        // Small counts at every marginal level, so the inline writers run.
        (6, vec![vec![1, 2], vec![-3, -4], vec![5, 6], vec![-1, 3], vec![4, -5]]),
        (6, vec![vec![1, 2, -3], vec![-2, 4], vec![3, 5, -6], vec![-1, -4, 5], vec![6, 2]]),
    ]);
    for (n, clauses) in &cases {
        let formula = make_formula(*n, clauses.clone());
        let expected = BigUint::from(brute_force_count(*n, clauses));
        for (shape, vtree) in &all_vtree_shapes(*n) {
            for (plan_name, plan) in [("detect", ComponentPlan::Detect), ("whole", ComponentPlan::Whole)] {
                let label = format!("{shape} vtree, {plan_name}, clauses {clauses:?}");
                let tdd = compile_cnf(eng, &formula, vtree, CompileOptions { output: Output::Count, plan, ..CompileOptions::default() })
                    .unwrap_or_else(|e| panic!("{label}: {e:?}"))
                    .diagram;
                assert_eq!(tdd.model_count().unwrap(), expected, "{label}");
                check_marginal_canonical_form(&tdd).unwrap_or_else(|e| panic!("{label}: {e}"));
                check_no_orphan_slots(&tdd).unwrap_or_else(|e| panic!("{label}: {e}"));
                check_inline_discipline(&tdd).unwrap_or_else(|e| panic!("{label}: {e}"));
                check_slot_count_uniqueness(&tdd).unwrap_or_else(|e| panic!("{label}: {e}"));
                let subsumed = subsumed_marginal_data_violations(&tdd);
                assert!(subsumed.is_empty(), "{label}: data left under a marginal parent at {subsumed:?}");
            }
        }
    }
}

/// A formula under `tests/data/` compiled on the FlowCutter primal vtree,
/// whose count must come out as `expected`.
fn check_fixture(fixture: &str, expected: &str) {
    let formula = load_cnf(&format!("tests/data/{fixture}"));
    let vtree = build_vtree(&formula, "flowcutter-primal", &VtreeOptions::default()).unwrap();
    let options = CompileOptions { output: Output::Count, plan: ComponentPlan::Detect, ..CompileOptions::default() };
    let tdd = compile_cnf(&Engine::new(), &formula, &vtree, options)
        .unwrap_or_else(|e| panic!("{fixture}: {e:?}"))
        .diagram;
    assert_eq!(tdd.model_count().unwrap().to_string(), expected, "{fixture}");
}

/// `fixture!(name, "file.cnf", "expected count")`, one `#[test]` per fixture
/// so cargo runs them in parallel.
macro_rules! fixture {
    ($name:ident, $file:literal, $expected:expr) => {
        #[test]
        fn $name() {
            check_fixture($file, $expected);
        }
    };
}

// The compile-level cascade passed grid slots through as inline counts,
// collapsing the count to 1.
fixture!(marg_canary_a, "marg_canary_a.cnf", "7");
fixture!(marg_toy_nr6, "marg_toy_nr6.cnf", "19");
// A bare slot-0 ref was read as the inline count 0.
fixture!(marg_toy_nr6b, "marg_toy_nr6b.cnf", "26");
fixture!(marg_toy_inline_nr, "marg_toy_inline_nr.cnf", "8");
fixture!(marg_toy_nr8_overcount, "marg_toy_nr8_overcount.cnf", "10");
// Model-counting competition instances whose counts run far past the inline
// width, so their larger marginal counts live in store slots.
fixture!(mc2020_track1_007, "cnfs/mcc/2020/track1/mc2020_track1_007.cnf", "428545652041973760");
fixture!(mc2024_track1_090, "cnfs/mcc/2024/track1/mc2024_track1_090.cnf", M090);
fixture!(mc2026_track1_041, "cnfs/mcc/2026/track1/mc2026_track1_041.cnf", "564153552511417968750");

const M090: &str = "348985067338598801901513742006521414111495136413120147089313831741045848974332480859174151657575340238037186953960953734480390700554411844905747668677922127872";
