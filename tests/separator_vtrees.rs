//! Hand-built vtrees around a separator. On synthetic CNFs where the variable
//! set `C` separates `A` from `B`, compile on ((A, C), B), (A, (C, B)) and
//! (C, (A, B)), each side balanced, and require every diagram to pass the
//! invariant checks and to count what the balanced vtree counts.

mod common;
use tididi::Engine;
use std::sync::Arc;

use vitri::cnf::{Clause, CnfFormula, Literal};
use tididi_bonsai::compile::{CompileOptions, compile_cnf};
use tididi::test_helpers::Lcg;
use tididi::test_helpers::check::check_all_fast;
use tididi::vtree::{VarId, Vtree};

/// A clause over `vars` (zero-based) with random polarities.
fn clause(vars: &[u32], rng: &mut Lcg) -> Clause {
    Clause::new(vars.iter().map(|&v| Literal::new(vitri::cnf::VarId::new(v + 1).unwrap(), rng.coin())).collect())
}

/// A clause over three distinct variables drawn from `pool`: the first three
/// places of a Fisher–Yates shuffle.
fn clause_from(pool: &[u32], rng: &mut Lcg) -> Clause {
    let mut buf = pool.to_vec();
    for i in 0..3 {
        let j = i + rng.below((buf.len() - i) as u64) as usize;
        buf.swap(i, j);
    }
    clause(&buf[..3], rng)
}

/// Random 3-CNF over `A` = 0..na, `C` = the next nc, `B` = the last nb: `ka`
/// clauses over A∪C and `kb` over C∪B, so `C` separates `A` from `B`.
fn synth_cut(na: u32, nc: u32, nb: u32, ka: usize, kb: usize, seed: u64) -> CnfFormula {
    let mut rng = Lcg::new(seed);
    let n = na + nc + nb;
    let a_pool: Vec<u32> = (0..na + nc).collect();
    let b_pool: Vec<u32> = (na..n).collect();
    let mut clauses: Vec<Clause> = (0..ka).map(|_| clause_from(&a_pool, &mut rng)).collect();
    clauses.extend((0..kb).map(|_| clause_from(&b_pool, &mut rng)));
    common::formula(n, clauses)
}

/// Two `rows`×`w` grids sharing a column, which is the separator `C`. Each
/// grid row carries three random 3-clauses, and vertically adjacent cells
/// (in either grid and in the shared column) a random 2-clause.
fn synth_two_grids(rows: u32, w: u32, seed: u64) -> (CnfFormula, Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut rng = Lcg::new(seed);
    let row_w = 2 * w - 1;
    let var_at = |r: u32, c: u32| r * row_w + c;

    let mut a_vars = Vec::new();
    let mut c_vars = Vec::new();
    let mut b_vars = Vec::new();
    for r in 0..rows {
        a_vars.extend((0..w - 1).map(|c| var_at(r, c)));
        c_vars.push(var_at(r, w - 1));
        b_vars.extend((0..w - 1).map(|c| var_at(r, w + c)));
    }

    let mut clauses: Vec<Clause> = Vec::new();
    for first_col in [0, w - 1] {
        for r in 0..rows {
            let pool: Vec<u32> = (0..w).map(|c| var_at(r, first_col + c)).collect();
            for _ in 0..3 {
                clauses.push(clause_from(&pool, &mut rng));
            }
        }
    }
    let columns = (0..w - 1).chain((0..w - 1).map(|c| w + c)).chain([w - 1]);
    for c in columns {
        for r in 0..rows.saturating_sub(1) {
            clauses.push(clause(&[var_at(r, c), var_at(r + 1, c)], &mut rng));
        }
    }

    (common::formula(rows * row_w, clauses), a_vars, c_vars, b_vars)
}

fn balanced(vars: &[u32]) -> Vtree {
    let ids: Vec<VarId> = vars.iter().map(|&v| VarId(v + 1)).collect();
    Vtree::balanced_over(&ids).unwrap()
}

fn join(left: &Vtree, right: &Vtree) -> Vtree {
    Vtree::join(left, right).unwrap()
}

fn check_cut_shapes(formula: &CnfFormula, a: &[u32], c: &[u32], b: &[u32]) {
    let eng = &Engine::new();
    let baseline = Arc::new(Vtree::balanced(formula.num_vars()));
    let baseline = compile_cnf(eng, formula, &baseline, CompileOptions::default()).unwrap().diagram;
    check_all_fast(&baseline, "balanced");
    let expected = baseline.model_count().unwrap();

    let (va, vc, vb) = (balanced(a), balanced(c), balanced(b));
    for (name, vtree) in [
        ("((A, C), B)", join(&join(&va, &vc), &vb)),
        ("(A, (C, B))", join(&va, &join(&vc, &vb))),
        ("(C, (A, B))", join(&vc, &join(&va, &vb))),
    ] {
        let tdd = compile_cnf(eng, formula, &Arc::new(vtree), CompileOptions::default()).unwrap().diagram;
        check_all_fast(&tdd, name);
        assert_eq!(tdd.model_count().unwrap(), expected, "{name}: the count differs from the balanced vtree's");
    }
}

#[test]
fn separator_shapes_on_random_cnfs() {
    // (|A|, |C|, |B|, clauses over A∪C, clauses over C∪B, seed)
    for (na, nc, nb, ka, kb, seed) in [
        (8, 2, 8, 25, 25, 42),
        (8, 4, 8, 25, 25, 42),
        (8, 6, 8, 25, 25, 42),
        (10, 3, 10, 35, 35, 42),
        (10, 5, 10, 35, 35, 42),
        (12, 4, 12, 40, 40, 42),
        (4, 8, 4, 20, 20, 11),
        (4, 10, 4, 25, 25, 11),
        (6, 8, 6, 30, 30, 11),
        (4, 12, 4, 30, 30, 11),
    ] {
        let formula = synth_cut(na, nc, nb, ka, kb, seed);
        let a: Vec<u32> = (0..na).collect();
        let c: Vec<u32> = (na..na + nc).collect();
        let b: Vec<u32> = (na + nc..na + nc + nb).collect();
        check_cut_shapes(&formula, &a, &c, &b);
    }
}

#[test]
fn separator_shapes_on_glued_grids() {
    for (rows, w) in [(3, 3), (3, 4), (4, 3), (4, 4)] {
        let (formula, a, c, b) = synth_two_grids(rows, w, 7);
        check_cut_shapes(&formula, &a, &c, &b);
    }
}
