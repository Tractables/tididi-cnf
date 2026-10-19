//! Compilation correctness. Every formula in the shared case table, seeded
//! random formulas, a few combinatorial problems and a formula padded with
//! don't-care variables are compiled on every vtree shape and checked against
//! their counts, `is_sat` and the library's invariant checks. Clause and
//! literal order must change nothing, and an ISCAS'89 circuit counts right on
//! its FlowCutter vtree and, with its inverters substituted away, on a saved
//! vtree.

mod common;

use std::sync::Arc;

use num_bigint::BigUint;
use vitri::cnf::CnfFormula;

use tididi::Engine;
use tididi::test_helpers::check::{check_all_fast, check_determinism};
use tididi::test_helpers::{CnfShape, Lcg, brute_force_count, check_minimize_soundness, rand_cnf};
use tididi::vtree::Vtree;
use tididi_cnf::compile::{CompileOptions, ComponentPlan, compile_cnf};
use tididi_cnf::vtree::{VtreeOptions, build_vtree};

use common::*;

/// Compiles `clauses` on every vtree shape and checks each diagram's count
/// against `expected`, its `is_sat` and `is_zero`, and the library's
/// invariant checks.
fn check_on_every_shape(eng: &Engine, num_vars: u32, clauses: &[Vec<i32>], expected: u64) {
    let formula = make_formula(num_vars, clauses.to_vec());
    for (shape, vtree) in &all_vtree_shapes(num_vars) {
        let label = format!("{shape} vtree, {num_vars} vars, clauses {clauses:?}");
        let mut tdd = compile_cnf(eng, &formula, vtree, CompileOptions::default()).unwrap().diagram;
        assert_eq!(tdd.model_count().unwrap(), BigUint::from(expected), "{label}");
        assert_eq!(tdd.is_sat().unwrap(), expected > 0, "{label}: is_sat");
        assert_eq!(tdd.is_zero(), expected == 0, "{label}: is_zero");
        check_all_fast(&tdd, &label);
        check_determinism(&tdd).unwrap_or_else(|e| panic!("{label}: determinism: {e}"));
        check_minimize_soundness(&mut tdd, 3).unwrap_or_else(|e| panic!("{label}: minimize soundness: {e}"));
    }
}

#[test]
fn the_case_table_compiles_right_on_every_shape() {
    let eng = &Engine::new();
    for (num_vars, clauses) in &case_table() {
        check_on_every_shape(eng, *num_vars, clauses, brute_force_count(*num_vars, clauses));
    }
}

/// Seeded, so a failure reproduces.
#[test]
fn random_formulas_compile_right_on_every_shape() {
    let eng = &Engine::new();
    let mut rng = Lcg::new(0x0B1CA0);
    for num_vars in [5u32, 6, 7] {
        for _ in 0..40 {
            let clauses = rand_cnf(&mut rng, num_vars, CnfShape { clauses: num_vars as usize + 4, width: 3 });
            check_on_every_shape(eng, num_vars, &clauses, brute_force_count(num_vars, &clauses));
        }
    }
}

/// Shuffling the clauses and the literals within each changes neither the
/// count nor the size.
#[test]
fn clause_and_literal_order_change_nothing() {
    let eng = &Engine::new();
    let mut rng = Lcg::new(0x70551E);
    let vtree = Arc::new(Vtree::balanced(5));
    let compile = |clauses: &[Vec<i32>]| {
        let tdd = compile_cnf(eng, &make_formula(5, clauses.to_vec()), &vtree, CompileOptions::default()).unwrap().diagram;
        (tdd.model_count().unwrap(), tdd.pair_count())
    };
    for _ in 0..20 {
        let clauses = rand_cnf(&mut rng, 5, CnfShape { clauses: 8, width: 3 });
        let mut shuffled = clauses.clone();
        shuffle(&mut shuffled, &mut rng);
        for clause in &mut shuffled {
            shuffle(clause, &mut rng);
        }
        assert_eq!(compile(&clauses), compile(&shuffled), "{clauses:?} against {shuffled:?}");
    }
}

fn shuffle<T>(items: &mut [T], rng: &mut Lcg) {
    for i in (1..items.len()).rev() {
        items.swap(i, rng.below(i as u64 + 1) as usize);
    }
}

/// At counts known without the brute-force oracle, which is checked too.
#[test]
fn combinatorial_problems_compile_right_on_every_shape() {
    let eng = &Engine::new();
    for (name, (num_vars, clauses), count) in [
        ("4-queens", queens_cnf(4), 2),
        ("2 pigeons, 2 holes", pigeonhole_cnf(2, 2), 2),
        ("3 pigeons, 2 holes", pigeonhole_cnf(3, 2), 0),
        ("3 pigeons, 3 holes", pigeonhole_cnf(3, 3), 6),
        ("4 pigeons, 3 holes", pigeonhole_cnf(4, 3), 0),
        ("3-colouring a triangle", triangle_colouring_cnf(), 6),
        ("2×2 Latin square", latin_square_cnf(2), 2),
    ] {
        assert_eq!(brute_force_count(num_vars, &clauses), count, "{name}");
        check_on_every_shape(eng, num_vars, &clauses, count);
    }
}

/// Each variable in no clause doubles the count.
#[test]
fn dont_care_variables_double_the_count() {
    let eng = &Engine::new();
    for extra in 0..=4u32 {
        check_on_every_shape(eng, 2 + extra, &[vec![1, 2], vec![-1, -2]], 2 << extra);
    }
}

/// Each node of a triangle gets exactly one of three colours, and adjacent
/// nodes differ: variable `3n + c + 1` gives node `n` colour `c`.
fn triangle_colouring_cnf() -> (u32, Vec<Vec<i32>>) {
    let var = |n: i32, c: i32| 3 * n + c + 1;
    let mut clauses: Vec<Vec<i32>> = (0..3).map(|n| (0..3).map(|c| var(n, c)).collect()).collect();
    for c1 in 0..3 {
        for c2 in c1 + 1..3 {
            clauses.extend((0..3).map(|n| vec![-var(n, c1), -var(n, c2)]));
        }
    }
    for (n1, n2) in [(0, 1), (1, 2), (0, 2)] {
        clauses.extend((0..3).map(|c| vec![-var(n1, c), -var(n2, c)]));
    }
    (9, clauses)
}

/// Each cell of an `n`×`n` grid holds exactly one of `n` values, none twice
/// in a row or a column: variable `n²r + nc + v + 1` puts `v` in cell `(r, c)`.
fn latin_square_cnf(n: i32) -> (u32, Vec<Vec<i32>>) {
    let var = |r: i32, c: i32, v: i32| n * n * r + n * c + v + 1;
    let mut clauses = Vec::new();
    for a in 0..n {
        for b in 0..n {
            clauses.push((0..n).map(|v| var(a, b, v)).collect());
            for x in 0..n {
                for y in x + 1..n {
                    // Cell (a, b) holds one value; value b appears once in row
                    // a and once in column a.
                    clauses.push(vec![-var(a, b, x), -var(a, b, y)]);
                    clauses.push(vec![-var(a, x, b), -var(a, y, b)]);
                    clauses.push(vec![-var(x, a, b), -var(y, a, b)]);
                }
            }
        }
    }
    ((n * n * n) as u32, clauses)
}

// ── A circuit instance ────────────────────────────────────────────────

/// The ISCAS'89 scan circuit s641, compiled whole on its FlowCutter incidence
/// vtree. Every structural invariant can hold on a diagram whose count is
/// wrong, so the count is pinned too.
#[test]
fn s641_counts_right_on_its_flowcutter_vtree() {
    let formula = load_cnf("tests/data/cnfs/iscas89/s641.scan.cnf");
    let vtree = build_vtree(&formula, "flowcutter-incidence", &VtreeOptions::default()).unwrap();
    let options = CompileOptions { plan: ComponentPlan::Whole, ..CompileOptions::default() };
    let tdd = compile_cnf(&Engine::new(), &formula, &vtree, options).unwrap().diagram;
    assert_eq!(tdd.model_count().unwrap(), BigUint::from(1u64) << 54, "s641");
    check_all_fast(&tdd, "s641");
}

// ── The same circuit without its inverters ───────────────────────────

/// The literal each of s641's 433 variables becomes, over 161 variables: an
/// inverter's output is its input negated, so an inverter's two clauses
/// become tautologies and are left out.
const S641_LITERALS: [i32; 433] = [
    158, 100, -11, 94, -18, 96, -20, 98, -22, 92, -29, -1, -6, -5, -7, 143, -23, 139, -21, 137,
    122, 128, -27, 125, -15, 123, 114, -2, 108, -25, 107, -17, 106, -28, 105, -24, 102, 160, 101, 159,
    -12, -9, -8, -10, -3, 90, -19, 89, -16, 88, -26, -4, 13, 14, 132, 134, 147, 153, 11, 161,
    133, 149, 154, 109, 115, 118, -79, 79, -79, 79, -79, 79, 157, -63, -53, 11, -11, -30, 30, -30,
    30, -30, 99, 18, -18, 4, -35, 35, -35, 93, 20, -20, 4, -36, 36, -36, 95, 22, -22, 4,
    -37, 37, -37, 97, 29, -29, 4, -34, 34, -34, 91, 156, 33, 155, -78, 78, -78, 78, 68, -74,
    74, -74, 74, -74, 74, 146, 152, 1, -1, 111, 110, -77, 77, -77, 145, 151, 6, -6, 127, 126,
    -76, 76, -76, 69, 144, 150, 5, -5, 117, 116, -75, 75, -75, 129, 33, -33, 33, -33, 68, -68,
    68, -68, 148, 7, -7, 141, 140, -73, 73, -73, 142, -72, 72, -72, 69, -69, -71, 71, 23, -23,
    138, -69, 69, -69, 69, -69, 21, -21, 67, -67, 67, -67, 67, -67, 67, -67, -68, 68, -68, -70,
    70, 48, 121, 130, 84, 80, -64, 64, -64, 64, -64, 64, -67, 67, 136, 31, 135, -66, 66, -66,
    66, 31, -31, 31, -31, 58, -58, 58, -58, 58, -65, 65, -65, 65, -65, 65, 59, 48, -48, 48,
    -48, 131, -131, 131, -61, 61, -61, 59, -59, -62, 62, 27, -27, 124, -59, 59, -59, 59, -59, 15,
    -15, 57, -57, 57, -57, 57, -57, 57, -57, -58, 58, -58, -60, 60, 9, 86, 46, 113, 82, -54,
    54, -54, 54, -54, 54, -57, 57, 120, 32, 119, -56, 56, -56, 56, 32, -32, 32, -32, 47, -47,
    47, -47, 47, -55, 55, -55, 55, -55, 55, 45, -45, 45, -45, 45, -45, 45, -45, 45, -45, 46,
    -46, 46, -46, 2, 112, -112, 112, 25, -25, 104, -48, 48, -48, -52, 52, 17, -17, 43, -43, 43,
    -43, 43, -43, 43, -43, -47, 47, -47, -51, 51, 28, -28, 103, -46, 46, -46, -50, 50, 24, -24,
    44, -44, 44, -44, 44, -44, -45, 45, -45, -49, 49, 87, 83, -39, 39, -39, 39, -39, 39, -44,
    44, 12, -12, 12, 9, -9, 8, -8, 8, 85, 10, -10, 10, 3, 81, -38, 38, -38, 38, -38,
    38, -43, 43, 19, -19, 4, -33, 33, -33, -42, 42, 16, -16, 4, -32, 32, -32, -41, 41, 26,
    -26, 4, -4, 4, -31, 31, -31, -40, 40, -13, 13, -14, 14,
];

/// s641's gates, each by the index of its long clause, which the circuit
/// lists ahead of one binary clause per input.
const S641_GATES: [usize; 107] = [
    836, 17, 39, 57, 768, 816, 800, 784, 515, 85, 346, 136, 518, 96, 355, 139, 10, 32, 50, 761,
    809, 793, 777, 825, 68, 119, 153, 181, 200, 232, 146, 174, 193, 225, 878, 711, 714, 246, 844, 349,
    874, 655, 658, 870, 319, 289, 324, 527, 329, 371, 402, 364, 684, 687, 425, 414, 643, 523, 433, 847,
    914, 891, 910, 488, 458, 493, 498, 542, 573, 535, 612, 615, 600, 585, 596, 670, 699, 725, 906, 622,
    886, 665, 694, 721, 902, 883, 753, 739, 758, 852, 81, 132, 342, 316, 511, 485, 608, 651, 680, 707,
    896, 750, 865, 184, 235, 405, 576,
];

/// s641 with its inverters substituted away: the gates in [`S641_GATES`]
/// order, each gate's binary clauses ahead of its long clause, over the
/// literals [`S641_LITERALS`] gives.
fn s641_without_inverters() -> CnfFormula {
    let circuit = load_cnf("tests/data/cnfs/iscas89/s641.scan.cnf");
    let clauses = circuit.clauses();
    let mut reduced = Vec::new();
    for &gate in &S641_GATES {
        let inputs = &clauses[gate + 1..gate + clauses[gate].literals.len()];
        for clause in inputs.iter().chain([&clauses[gate]]) {
            let mut literals: Vec<i32> = clause.literals.iter()
                .map(|l| if l.positive { S641_LITERALS[l.var.idx()] } else { -S641_LITERALS[l.var.idx()] })
                .collect();
            literals.sort_by_key(|l| l.abs());
            reduced.push(literals);
        }
    }
    make_formula(161, reduced)
}

/// On this saved FlowCutter incidence vtree, a sort of seven child pairs that
/// leaves two equal pairs apart keeps them both, and the diagram passes every
/// structural check yet counts 2^54 + 205 · 2^29.
#[test]
fn s641_without_inverters_counts_right_on_a_saved_vtree() {
    let formula = s641_without_inverters();
    let vtree = load_vtree("tests/data/s641_without_inverters.vtree");
    let tdd = compile_cnf(&Engine::new(), &formula, &vtree, CompileOptions::default()).unwrap().diagram;
    assert_eq!(tdd.model_count().unwrap(), BigUint::from(1u64) << 54, "s641 without inverters");
    check_all_fast(&tdd, "s641 without inverters");
}
