//! The marginalizing compile (`Output::Count`, the model count's compile).
//! Every formula of the shared case table, plus a few with separated
//! components and small counts, is compiled on every vtree shape and must
//! count right and pass the library's marginal checks; and a set of saved
//! formulas, small ones whose marginal counts sit in slots and competition
//! instances, must count right, as must two formulas built here that once
//! counted wrong: a slice of a competition instance, and a long clause split
//! by a chain of auxiliaries on a saved vtree.

mod common;

use std::ops::Range;

use num_bigint::BigUint;
use vitri::cnf::CnfFormula;

use tididi::Engine;
use tididi::test_helpers::brute_force_count;
use tididi::test_helpers::check::marginal::{
    check_inline_discipline, check_marginal_canonical_form, check_no_orphan_slots,
    check_slot_count_uniqueness, subsumed_marginal_data_violations,
};
use tididi_bonsai::compile::{CompileOptions, ComponentPlan, Output, compile_cnf};
use tididi_bonsai::vtree::{VtreeOptions, build_vtree};

use common::{all_vtree_shapes, case_table, load_cnf, load_vtree, make_formula};

#[test]
fn marginalizing_compile_is_canonical_on_every_shape() {
    let eng = &Engine::new();
    let mut cases = case_table();
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

/// `formula`'s marginalizing compile, whose count must come out as
/// `expected`. A saved vtree under `tests/data/` reproduces a failure only on
/// itself, so the formula compiles whole on it; without one it compiles on the
/// FlowCutter primal vtree, its components detected.
fn check_count(name: &str, formula: &CnfFormula, saved_vtree: Option<&str>, expected: &str) {
    let (vtree, plan) = match saved_vtree {
        Some(file) => (load_vtree(&format!("tests/data/{file}")), ComponentPlan::Whole),
        None => (build_vtree(formula, "flowcutter-primal", &VtreeOptions::default()).unwrap(), ComponentPlan::Detect),
    };
    let options = CompileOptions { output: Output::Count, plan, ..CompileOptions::default() };
    let tdd = compile_cnf(&Engine::new(), formula, &vtree, options)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"))
        .diagram;
    assert_eq!(tdd.model_count().unwrap().to_string(), expected, "{name}");
}

/// The formula under `tests/data/` at `fixture`, through [`check_count`].
fn check_fixture(fixture: &str, expected: &str) {
    check_count(fixture, &load_cnf(&format!("tests/data/{fixture}")), None, expected);
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

// Small formulas whose marginal counts sit in store slots beside inline
// ones: each reference reads the count it names, a slot's from the store
// and an inline one from the reference itself.
fixture!(marginal_one_clause, "marginal_one_clause.cnf", "7");
fixture!(marginal_slots_beside_inline, "marginal_slots_beside_inline.cnf", "19");
// A reference to slot 0 reads the slot, not the inline count 0.
fixture!(marginal_slot_zero, "marginal_slot_zero.cnf", "26");
fixture!(marginal_inline_decode, "marginal_inline_decode.cnf", "8");
fixture!(marginal_with_unit_clauses, "marginal_with_unit_clauses.cnf", "10");
// Model-counting competition instances whose counts run far past the inline
// width, so their larger marginal counts live in store slots.
fixture!(mc2020_track1_007, "cnfs/mcc/2020/track1/mc2020_track1_007.cnf", "428545652041973760");
fixture!(mc2024_track1_090, "cnfs/mcc/2024/track1/mc2024_track1_090.cnf", M090);
fixture!(mc2026_track1_041, "cnfs/mcc/2026/track1/mc2026_track1_041.cnf", "564153552511417968750");

const M090: &str = "348985067338598801901513742006521414111495136413120147089313831741045848974332480859174151657575340238037186953960953734480390700554411844905747668677922127872";

/// The clauses of `mc2021_track1_139` in these index ranges, over its first
/// 1489 variables.
const MC2021_139_SLICE: [Range<usize>; 20] = [
    698..734, 738..794, 798..814, 818..830, 834..842, 846..850, 854..858, 862..878, 882..962, 966..1086,
    1090..1118, 1122..1150, 1154..1370, 1374..1477, 1606..1791, 1795..1993, 2122..2132, 2136..2216, 2220..2436, 2440..3560,
];

const M139: &str = "97323534971334692867413752585300045581622376739375978395637495115662951127143796783196748902314370365987450253302914821042297245891173632182678688949966552773471278344659622542572797437233070080";

// Disjoint marginal subfunctions of this slice share their counts, which run
// past the inline width: two such children read as one slot lose a count.
#[test]
fn mc2021_track1_139_slice() {
    let instance = load_cnf("tests/data/cnfs/mcc/2021/track1/mc2021_track1_139.cnf");
    let clauses = MC2021_139_SLICE.iter().flat_map(|range| instance.clauses()[range.clone()].iter().cloned()).collect();
    let formula = common::formula(1489, clauses);
    check_count("mc2021_track1_139 slice", &formula, None, M139);
}

/// The clause x1 ∨ … ∨ x32 ∨ z1 ∨ … ∨ z32 over variables 1 to 64, split by a
/// chain of 31 auxiliaries: y_k, variable 64 + k, holds exactly when none of
/// x1 to x_(k+1) does, and y31 needs some z. Each clause lists its literals by
/// variable.
fn chained_wide_clause() -> CnfFormula {
    let (x, z, y) = (|i: i32| i, |i: i32| 32 + i, |k: i32| 64 + k);
    let mut clauses: Vec<Vec<i32>> = (1..=32).map(|i| vec![-x(i), -y((i - 1).max(1))]).collect();
    clauses.extend((1..=30).map(|k| vec![y(k), -y(k + 1)]));
    clauses.push(vec![x(1), x(2), y(1)]);
    clauses.extend((1..=30).map(|k| vec![x(k + 2), -y(k), y(k + 1)]));
    clauses.push((1..=32).map(z).chain([-y(31)]).collect());
    make_formula(95, clauses)
}

// On this saved FlowCutter incidence vtree, a clause apply that rebuilds a
// level without its inline-count markers lets the count's readout take inline
// counts for slot indices, and the count comes out half again too high.
#[test]
fn chained_wide_clause_on_a_saved_vtree() {
    check_count("chained wide clause", &chained_wide_clause(), Some("chained_wide_clause.vtree"), "18446744073709551615");
}
