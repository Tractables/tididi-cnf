//! Shared test utilities for integration tests: formula builders and CNF
//! loading, brute-force counters, the case table, the vtree shapes and the
//! n-queens, pigeonhole and random 3-CNF generators.

// A grab-bag `mod common;` pulled into many test binaries; any one binary
// uses only a slice of it.
#![allow(dead_code, unused_imports)]

use std::collections::HashSet;
use std::sync::Arc;

use num_bigint::BigUint;
use num_rational::BigRational;
use vitri::cnf::{Clause, CnfFormula, Literal};
use vitri::cnf::VarId;
use tididi::vtree::Vtree;

/// Parse the DIMACS file at `path`, relative to the crate root.
pub fn load_cnf(path: &str) -> CnfFormula {
    let path = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    tididi_bonsai::parse_dimacs(std::io::BufReader::new(file))
        .unwrap_or_else(|e| panic!("parse {path}: {e}"))
        .0
}

/// A path in the temp directory, unique to this test process.
pub fn tmp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("tididi_test_{}_{name}", std::process::id()))
}

/// A test formula over `num_vars` variables, which the fixture is expected to
/// stay within.
pub fn formula(num_vars: u32, clauses: Vec<Clause>) -> CnfFormula {
    CnfFormula::new(num_vars, clauses).expect("a test formula names only variables in its space")
}

pub fn make_formula(num_vars: u32, clauses: Vec<Vec<i32>>) -> CnfFormula {
    let clauses = clauses
        .into_iter()
        .map(|lits| {
            Clause::new(
                lits.into_iter()
                    .map(|l| Literal::new(VarId::from_dimacs(l), l > 0))
                    .collect(),
            )
        })
        .collect();
    formula(num_vars, clauses)
}

/// The models of a small formula, each a bitmask with bit `i` holding
/// zero-based variable `i`.
pub fn models(formula: &CnfFormula) -> impl Iterator<Item = u64> + '_ {
    assert!(formula.num_vars() <= 22, "keep brute-force test formulas tiny");
    (0u64..1 << formula.num_vars()).filter(move |&assignment| {
        formula.clauses().iter().all(|clause| {
            clause.literals.iter().any(|literal| ((assignment >> literal.var.idx()) & 1 == 1) == literal.positive)
        })
    })
}

/// Brute-force projected model count onto `show` (zero-based variable indices).
pub fn brute_force_projected_count(formula: &CnfFormula, show: &[u32]) -> BigUint {
    let mask = show.iter().fold(0u64, |mask, &v| mask | 1 << v);
    BigUint::from(models(formula).map(|model| model & mask).collect::<HashSet<u64>>().len())
}

/// Brute-force weighted model count: the sum over the models of the product
/// of their literals' weights. `w[v]` is `(w_neg, w_pos)` for zero-based
/// variable `v`.
pub fn brute_force_wmc(formula: &CnfFormula, w: &[(BigRational, BigRational)]) -> BigRational {
    assert_eq!(w.len(), formula.num_vars() as usize, "one weight pair per variable");
    models(formula)
        .map(|model| w.iter().enumerate().map(|(v, (neg, pos))| if (model >> v) & 1 == 1 { pos } else { neg }).product::<BigRational>())
        .sum()
}

pub fn get_test_cases() -> Vec<(u32, Vec<Vec<i32>>)> {
    vec![
        // --- Basic small formulas ---
        (3, vec![vec![1, 2], vec![-2, 3], vec![-1, -3]]),
        (4, vec![vec![1, 2], vec![3, 4], vec![-1, -3], vec![-2, -4]]),
        (4, vec![vec![1], vec![2, -3], vec![-1, 3, 4], vec![-2, -4]]),
        (3, vec![vec![1, 2, 3]]),
        (2, vec![vec![1, 2], vec![-1, -2], vec![1, -2], vec![-1, 2]]),
        // --- UNSAT cases ---
        (1, vec![vec![1], vec![-1]]),
        (2, vec![vec![1], vec![2], vec![-1, -2]]),
        (3, vec![vec![1], vec![-1], vec![2, 3]]),
        (4, vec![vec![1, 2], vec![-1, -2], vec![1, -2], vec![-1, 2],
               vec![3, 4], vec![-3, -4], vec![3, -4], vec![-3, 4]]),
        // --- Single literal ---
        (3, vec![vec![1]]),
        (3, vec![vec![-2]]),
        // --- All variables in one clause ---
        (4, vec![vec![1, 2, 3, 4]]),
        // --- Multiple clauses, some variables unused in some ---
        (4, vec![vec![1, 2], vec![3], vec![-4]]),
        // --- Don't-care variables ---
        (4, vec![vec![1, 2], vec![-1, -2]]),
        (5, vec![vec![1]]),
        (3, vec![vec![1, 2], vec![-1, -2], vec![3]]),
        (6, vec![vec![1, 2], vec![-3, -4]]),
        // --- Pigeonhole ---
        (6, vec![
            vec![1, 2], vec![3, 4], vec![5, 6],
            vec![-1, -3], vec![-1, -5], vec![-3, -5],
            vec![-2, -4], vec![-2, -6], vec![-4, -6],
        ]),
        // --- Chain implications ---
        (5, vec![vec![-1, 2], vec![-2, 3], vec![-3, 4], vec![-4, 5]]),
        (7, vec![vec![-1, 2], vec![-2, 3], vec![-3, 4], vec![-4, 5],
                 vec![-5, 6], vec![-6, 7]]),
        // --- Biconditional chain ---
        (4, vec![vec![-1, 2], vec![1, -2], vec![-2, 3], vec![2, -3],
                 vec![-3, 4], vec![3, -4]]),
        // --- At-most-one ---
        (4, vec![vec![-1, -2], vec![-1, -3], vec![-1, -4],
                 vec![-2, -3], vec![-2, -4], vec![-3, -4]]),
        // --- Exactly-one ---
        (4, vec![vec![1, 2, 3, 4],
                 vec![-1, -2], vec![-1, -3], vec![-1, -4],
                 vec![-2, -3], vec![-2, -4], vec![-3, -4]]),
        // --- Parity / XOR ---
        (2, vec![vec![1, 2], vec![-1, -2]]),
        (3, vec![vec![1, 2, 3], vec![-1, -2, 3], vec![-1, 2, -3], vec![1, -2, -3]]),
        // --- 2-SAT ---
        (4, vec![vec![1, 2], vec![-1, 3], vec![-2, 4], vec![-3, -4]]),
        (5, vec![vec![1, 2], vec![-2, 3], vec![-3, 4], vec![-4, 5], vec![-5, -1]]),
        // --- Horn clauses ---
        (4, vec![vec![-1, -2, 3], vec![-3, 4], vec![-4, -1], vec![1]]),
        (5, vec![vec![1], vec![-1, 2], vec![-2, 3], vec![-1, -3, 4], vec![-4, 5]]),
        // --- Mixed clause widths ---
        (6, vec![vec![1], vec![2, 3], vec![-1, -2, -3], vec![4, 5, 6],
                 vec![-4, -5], vec![-5, -6]]),
        // --- Over-constrained ---
        (3, vec![vec![1, 2], vec![1, 3], vec![2, 3],
                 vec![-1, -2], vec![-1, -3], vec![-2, -3],
                 vec![1, 2, 3]]),
        // --- Wide clauses ---
        (8, vec![vec![1, 2, 3, 4, 5, 6, 7, 8]]),
        (8, vec![vec![1, 2, 3, 4, 5, 6, 7, 8],
                 vec![-1, -2, -3, -4, -5, -6, -7, -8]]),
        // --- Disjoint variable groups ---
        (6, vec![vec![1, 2], vec![-1, -2], vec![3, 4], vec![-3, -4],
                 vec![5, 6], vec![-5, -6]]),
        // --- Single variable ---
        (1, vec![vec![1]]),
        (1, vec![vec![-1]]),
        // --- No clauses, and a few small mixed formulas ---
        (3, vec![]),
        (3, vec![vec![1, -2]]),
        (3, vec![vec![1], vec![2]]),
        (3, vec![vec![1, 2, 3], vec![-1, -2, 3]]),
        (3, vec![vec![1, 2], vec![-1, 3]]),
        (3, vec![vec![1, 2], vec![-1, -2], vec![2, 3]]),
        (4, vec![vec![1], vec![2], vec![-3]]),
        (4, vec![vec![1, 2], vec![-1, 3], vec![-2, 4]]),
        (4, vec![vec![1, 2], vec![-1, 3], vec![-2, 4], vec![1, -3]]),
        (4, vec![vec![1, 2, 3], vec![-1, -2], vec![-2, 4], vec![-3, -4]]),
        (5, vec![vec![1, 2], vec![-2, 3], vec![-3, 4], vec![4, 5]]),
        (6, vec![vec![1, 6]]),
        (10, vec![vec![1, 2, 3], vec![-1, 4, 5], vec![-2, 6, 7], vec![-3, 8, 9], vec![4, -5, 10]]),
        // --- A repeated clause ---
        (4, vec![vec![1, 2], vec![1, 2], vec![-1, 3], vec![-2, 4]]),
        // --- Independent components, some UNSAT ---
        (4, vec![vec![1, 2], vec![3, 4]]),
        (4, vec![vec![1, 2], vec![-1, 2], vec![3, 4], vec![-3, 4]]),
        (6, vec![vec![1, 2], vec![3, 4], vec![5, 6]]),
        (6, vec![vec![1], vec![2, 3], vec![-2, 3], vec![4, 5, 6]]),
        (6, vec![vec![1, 2], vec![-1, 3], vec![4, 5, 6]]),
        (4, vec![vec![1, 2], vec![3], vec![-3]]),
        (4, vec![vec![1], vec![-1], vec![3, 4]]),
        // --- UNSAT among clauses over the same variables ---
        (2, vec![vec![1, 2], vec![-1], vec![-2]]),
        (2, vec![vec![1], vec![-1, 2], vec![-2], vec![-1]]),
        // --- Two blocks of three with pairwise exclusions ---
        (6, vec![vec![1, 2, 3], vec![4, 5, 6], vec![-1, -4], vec![-2, -5], vec![-3, -6],
                 vec![-1, -2], vec![-4, -5]]),
        // --- Many clauses over the same variables ---
        (4, vec![vec![1, 2, 3, 4], vec![-1, 2, 3], vec![1, -2, 4], vec![-3, -4],
                 vec![2, 3], vec![-1, -4], vec![1, 3, 4]]),
        (6, (1..=6).flat_map(|i| (1..=6).filter(move |&j| j != i).map(move |j| vec![i, j])).collect()),
        (8, (1..=7)
            .flat_map(|i| (i + 1..=8).map(move |j| vec![i, j]))
            .chain([vec![-1, -2, -3], vec![-4, -5, -6], vec![-6, -7, -8],
                    vec![1, 3, 5, 7], vec![2, 4, 6, 8]])
            .collect()),
        (10, (1..=8)
            .flat_map(|i| (i + 1..=9).flat_map(move |j| [vec![i, j], vec![-i, j]]))
            .collect()),
    ]
}

/// The vtree shapes the tests compile on, by the names [`vtree_shape`] takes.
pub const VTREE_SHAPES: [&str; 7] =
    ["balanced", "linear", "reverse_linear", "random0", "random1", "random42", "interleaved"];

/// The vtree shape `name` over `num_vars` variables. `interleaved` is the
/// linear vtree over the even variables, then the odd ones.
pub fn vtree_shape(name: &str, num_vars: u32) -> Arc<Vtree> {
    use tididi::vtree::VarId;
    Arc::new(match name {
        "balanced" => Vtree::balanced(num_vars),
        "linear" => Vtree::linear(num_vars),
        "reverse_linear" => Vtree::reverse_linear(num_vars),
        "random0" => Vtree::random(num_vars, 0),
        "random1" => Vtree::random(num_vars, 1),
        "random42" => Vtree::random(num_vars, 42),
        "interleaved" => {
            let order: Vec<VarId> = (2..=num_vars).step_by(2).chain((1..=num_vars).step_by(2)).map(VarId).collect();
            Vtree::linear_from_order(&order).unwrap()
        }
        _ => panic!("no vtree shape {name}"),
    })
}

/// Every shape of [`VTREE_SHAPES`] over `num_vars` variables, with its name.
pub fn all_vtree_shapes(num_vars: u32) -> Vec<(&'static str, Arc<Vtree>)> {
    VTREE_SHAPES.iter().map(|&name| (name, vtree_shape(name, num_vars))).collect()
}

/// `run()` under a stop on `eng` that cuts whatever it runs there once the
/// engine's work clock has advanced `work` units: a step count, so the cut
/// lands at the same point on every machine.
pub fn cut_after<T>(eng: &tididi::Engine, work: u64, run: impl FnOnce() -> T) -> T {
    use tididi::limits::{StopCallback, StopDecision};
    let at = eng.limits().work_units() + work;
    let _cut = eng.limits().edit(|config| {
        config.with_stop_callback(Some(StopCallback::new(move |meters, _| {
            if meters.work_units >= at { StopDecision::Stop } else { StopDecision::Continue }
        })))
    });
    run()
}

/// N-queens: one queen per row and column, none sharing a diagonal;
/// variable `n·r + c + 1` puts a queen on square `(r, c)`.
pub fn queens_cnf(n: i32) -> (u32, Vec<Vec<i32>>) {
    let num_vars = (n * n) as u32;
    let var = |r: i32, c: i32| -> i32 { r * n + c + 1 };
    let mut clauses: Vec<Vec<i32>> = Vec::new();

    for r in 0..n {
        clauses.push((0..n).map(|c| var(r, c)).collect());
    }
    for r in 0..n {
        for c1 in 0..n {
            for c2 in (c1 + 1)..n {
                clauses.push(vec![-var(r, c1), -var(r, c2)]);
            }
        }
    }
    for c in 0..n {
        for r1 in 0..n {
            for r2 in (r1 + 1)..n {
                clauses.push(vec![-var(r1, c), -var(r2, c)]);
            }
        }
    }
    for r1 in 0..n {
        for c1 in 0..n {
            for r2 in (r1 + 1)..n {
                for c2 in 0..n {
                    if (r1 - r2).abs() == (c1 - c2).abs() {
                        clauses.push(vec![-var(r1, c1), -var(r2, c2)]);
                    }
                }
            }
        }
    }
    (num_vars, clauses)
}

/// Each pigeon in some hole, no two in one: variable `holes·p + h + 1` puts
/// pigeon `p` in hole `h`.
pub fn pigeonhole_cnf(pigeons: i32, holes: i32) -> (u32, Vec<Vec<i32>>) {
    let var = |p: i32, h: i32| p * holes + h + 1;
    let mut clauses: Vec<Vec<i32>> = (0..pigeons).map(|p| (0..holes).map(|h| var(p, h)).collect()).collect();
    for h in 0..holes {
        for p1 in 0..pigeons {
            for p2 in p1 + 1..pigeons {
                clauses.push(vec![-var(p1, h), -var(p2, h)]);
            }
        }
    }
    ((pigeons * holes) as u32, clauses)
}

/// A uniform random 3-CNF: `num_clauses` clauses, each over three distinct
/// variables with random signs, drawn from the library's seeded generator so
/// that a seed names the same formula on every platform.
pub fn random_3cnf(num_vars: u32, num_clauses: usize, seed: u64) -> CnfFormula {
    let mut rng = tididi::test_helpers::Lcg::new(seed);
    let clauses = (0..num_clauses)
        .map(|_| {
            let mut vars: Vec<i32> = Vec::with_capacity(3);
            while vars.len() < 3 {
                let v = rng.below(u64::from(num_vars)) as i32 + 1;
                if !vars.contains(&v) {
                    vars.push(v);
                }
            }
            vars.sort_unstable();
            vars.into_iter().map(|v| if rng.coin() { v } else { -v }).collect()
        })
        .collect();
    make_formula(num_vars, clauses)
}
