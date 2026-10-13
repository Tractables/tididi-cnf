//! Compilation correctness. Every formula in the shared case table, seeded
//! random formulas, a few combinatorial problems and a formula padded with
//! don't-care variables are compiled on every vtree shape and checked against
//! their counts, `is_sat` and the library's invariant checks. Clause and
//! literal order must change nothing, and an ISCAS'89 circuit counts right on
//! its FlowCutter vtree.

mod common;

use tididi::Engine;
use std::sync::Arc;

use num_bigint::BigUint;
use tididi_bonsai::compile::{CompileOptions, ComponentPlan, compile_cnf};
use tididi_bonsai::vtree::{VtreeOptions, build_vtree};
use tididi::test_helpers::check::{check_all_fast, check_determinism};
use tididi::test_helpers::check_minimize_soundness;
use tididi::test_helpers::{CnfShape, Lcg, brute_force_count, rand_cnf};
use tididi::vtree::Vtree;

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
    for (num_vars, clauses) in &get_test_cases() {
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
