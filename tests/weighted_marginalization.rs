//! Weighted marginalization against the full-diagram oracle: a plain
//! `compile_cnf` evaluated under the same weights, which shares no
//! marginal-store code with it. Covers the weighted marginalizing compile
//! (`Output::WeightedCount`, the compile the binary's weighted count runs),
//! and weighted marginalization of a plain compile by any set of variables.
//!
//! The fuzz test is ignored and reads its ranges from the environment, and
//! refuses a value it cannot read:
//!
//! - `TIDIDI_FUZZ_SEED_LO`, `TIDIDI_FUZZ_SEED_HI`: the seed range (default
//!   0..400);
//! - `TIDIDI_FUZZ_NVARS_LO`, `TIDIDI_FUZZ_NVARS_HI`: variables per instance
//!   (default 6..=18);
//! - `TIDIDI_FUZZ_DENS_LO`, `TIDIDI_FUZZ_DENS_HI`: clauses per variable
//!   ×100 (default 150..=400);
//! - `TIDIDI_FUZZ_WEIGHTS`: `spread` (the default), `posfrac` or `equalish`;
//! - `TIDIDI_FUZZ_STOP`: stop after this many mismatches (default 1).

mod common;

use std::sync::Arc;

use num_rational::BigRational;
use num_traits::{One, Zero};
use vitri::cnf::CnfFormula;

use tididi::Engine;
use tididi::diagram::{Arithmetic, Tdd};
use tididi::test_helpers::{Lcg, exact_weight, rat};
use tididi::vtree::{Home, VarId, Vtree};
use tididi_cnf::compile::{CompileOptions, ComponentPlan, Output, compile_cnf};
use tididi_cnf::vtree::{VtreeOptions, build_vtree};

use common::{make_formula, random_3cnf};

/// `(w_neg, w_pos)` per variable, the order `rational_weight_algebra` takes.
type Weights = Vec<(BigRational, BigRational)>;

/// The library's weighted-counting algebra over `(w_neg, w_pos)` pairs, as the
/// compiler builds it from a resolved weight table.
fn rational_weight_algebra(weights: &[(BigRational, BigRational)]) -> tididi::diagram::RationalWeights {
    let (negative, positive) = weights.iter().cloned().unzip();
    tididi::diagram::RationalWeights::from_polarities(tididi::diagram::LiteralWeights { negative, positive })
        .expect("both polarity columns come from the same variable pairs")
}

/// A plain compile of `formula` on `vtree` as one component, so that the
/// diagram is on a home made of `vtree`.
fn plain_compile(eng: &Engine, formula: &CnfFormula, vtree: &Arc<Vtree>) -> Tdd {
    compile_cnf(eng, formula, vtree, CompileOptions { plan: ComponentPlan::Whole, ..CompileOptions::default() }).unwrap().diagram
}

/// The oracle: a plain compile evaluated under `w`.
fn oracle(formula: &CnfFormula, vtree: &Arc<Vtree>, w: &[(BigRational, BigRational)]) -> BigRational {
    plain_compile(&Engine::new(), formula, vtree)
        .evaluate(&rational_weight_algebra(w))
        .unwrap()
}

/// The marginalizing compile's count. Its verdict must be the formula's,
/// whatever the weights: unsatisfiable exactly when a plain compile has no
/// model, and satisfiable when the count is zero only through its weights.
fn marginalizing(formula: &CnfFormula, vtree: &Arc<Vtree>, w: &[(BigRational, BigRational)]) -> BigRational {
    let tdd = weighted_mc_compile(&Engine::new(), formula, vtree, w);
    let satisfiable = plain_compile(&Engine::new(), formula, vtree).is_sat().unwrap();
    // A weighted compile stores no unsatisfiable node, so it answers
    // unsatisfiable exactly when it returns the false diagram.
    assert_eq!(!tdd.is_zero(), satisfiable, "the weighted compile's verdict is not the formula's");
    if tdd.is_zero() { BigRational::zero() } else { weighted_value(&tdd) }
}

/// The marginalizing compile with `w` attached, as a diagram.
fn weighted_mc_compile(eng: &Engine, formula: &CnfFormula, vtree: &Arc<Vtree>, w: &[(BigRational, BigRational)]) -> Tdd {
    compile_cnf(
        eng,
        formula,
        vtree,
        CompileOptions {
            output: Output::WeightedCount { weights: rational_weight_algebra(w), arithmetic: Arithmetic::ExactRational },
            ..CompileOptions::default()
        },
    )
    .expect("no deadline is armed")
    .diagram
}

/// Move `tdd` onto a copy of its home weighted by `w` and sum out the
/// variables `summed` names in one batch.
fn marginalize_weighted(eng: &Engine, tdd: &mut Tdd, w: &[(BigRational, BigRational)], summed: impl Fn(VarId) -> bool) {
    let weights = tididi::diagram::Weights::new(rational_weight_algebra(w), Arithmetic::ExactRational);
    let home = tdd.home();
    let weighted = Home::weighted(eng.bind_vtree(Vtree::clone(&home)), weights).expect("the weights cover the home");
    *tdd = eng.transport(&*tdd, &weighted, |v| v, 0).expect("the same home, weighted").0;
    eng.marginalize(tdd, summed).expect("no wall is armed");
}

fn weighted_value(tdd: &Tdd) -> BigRational {
    exact_weight(&tdd.weighted_value().unwrap().expect("a weighted diagram has a value"))
}

/// The oracle against brute force, then both weighted paths against the
/// oracle: the marginalizing compile, and a bottom-up weighted
/// marginalization of every level of a plain compile.
#[test]
fn weighted_counts_match_the_oracle() {
    let eng = &Engine::new();
    let weights = |pairs: &[(i64, i64, i64, i64)]| -> Weights { pairs.iter().map(|&(a, b, c, d)| (rat(a, b), rat(c, d))).collect() };
    for (n, clauses, w) in [
        (3, vec![vec![1, 2], vec![-2, 3], vec![1, -3]], weights(&[(1, 1, 1, 1); 3])),
        (5, vec![vec![1, 2], vec![-2, 3], vec![4, -5]], weights(&[(1, 1, 1, 1); 5])),
        // 2/3·3/5 + 2/3·2/5 + 1/3·3/5 = 13/15.
        (2, vec![vec![1, 2]], weights(&[(1, 3, 2, 3), (2, 5, 3, 5)])),
        (4, vec![vec![1, 2], vec![-2, 3], vec![1, -4]], weights(&[(1, 3, 2, 3), (2, 5, 3, 5), (1, 1, 4, 1), (3, 7, 4, 7)])),
        // Satisfiable formulas that weigh 0: a weight of zero on both sides of
        // x1, and a free variable whose signed weights cancel.
        (2, vec![vec![1, 2]], weights(&[(0, 1, 0, 1), (1, 1, 1, 1)])),
        (1, vec![], weights(&[(-1, 1, 1, 1)])),
        // x3 is in no clause and contributes w⁺ + w⁻.
        (3, vec![vec![1, 2]], weights(&[(1, 2, 1, 3), (2, 5, 3, 7), (1, 1, 4, 9)])),
        (4, vec![vec![1, 2], vec![-2, 3]], weights(&[(1, 2, 1, 3), (2, 5, 3, 7), (5, 11, 2, 9), (1, 1, 4, 9)])),
        (
            6,
            vec![vec![1, 2, -3], vec![-2, 4], vec![3, 5, -6], vec![-1, -4, 5], vec![6, 2]],
            weights(&[(1, 2, 1, 3), (2, 5, 3, 7), (5, 11, 2, 9), (7, 4, 1, 6), (3, 8, 5, 13), (9, 10, 4, 9)]),
        ),
        // Three separated components, all satisfiable.
        (
            8,
            vec![vec![1, 2], vec![-1, -2], vec![3, 4], vec![4, 5], vec![6, 7], vec![-7, 8]],
            weights(&[(9, 10, 1, 10), (8, 10, 2, 10), (7, 10, 3, 10), (6, 10, 4, 10), (5, 10, 5, 10), (4, 10, 6, 10), (3, 10, 7, 10), (2, 10, 8, 10)]),
        ),
        // Three separated components, the middle one unsatisfiable: the whole
        // formula weighs 0.
        (
            4,
            vec![vec![1, 2], vec![-1, -2], vec![3], vec![-3], vec![4]],
            weights(&[(9, 10, 1, 10), (8, 10, 2, 10), (7, 10, 3, 10), (6, 10, 4, 10)]),
        ),
        // Forced by a unit clause, the one variable leaves a leaf output,
        // which has no stored weight slot to read.
        (1, vec![vec![1]], weights(&[(1, 3, 2, 3)])),
    ] {
        let formula = make_formula(n, clauses.clone());
        let vtree = Arc::new(Vtree::balanced(n));
        let expected = oracle(&formula, &vtree, &w);
        assert_eq!(expected, common::brute_force_wmc(&formula, &w), "the oracle, {clauses:?}");
        let mut plain = plain_compile(eng, &formula, &vtree);
        if w.iter().all(|(neg, pos)| neg.is_one() && pos.is_one()) {
            assert_eq!(expected, BigRational::from_integer(plain.model_count().unwrap().into()), "unit weights count models");
        }
        assert_eq!(marginalizing(&formula, &vtree, &w), expected, "marginalizing compile, {clauses:?}");
        marginalize_weighted(eng, &mut plain, &w, |_| true);
        assert_eq!(weighted_value(&plain), expected, "marginalized plain compile, {clauses:?}");
    }
}

/// Under unit weights the weighted count is the model count, 30. On this
/// formula and its min-fill vtree, a marginalizing compile that drops the
/// mass of some models anywhere comes out short.
#[test]
fn weighted_marginalizing_compile_keeps_all_model_mass() {
    let formula = make_formula(8, vec![
        vec![-1, 7, -8],
        vec![2, 6, 8],
        vec![-3, 7],
        vec![3, -6, 8],
        vec![-2, 4, -5],
        vec![-2, -3, -5],
        vec![-4, -6],
        vec![4, 7],
        vec![3, -4],
        vec![-5, -7],
    ]);
    let vtree = build_vtree(&formula, "minfill-primal", &VtreeOptions::default()).unwrap();
    let w: Weights = (0..8).map(|_| (BigRational::one(), BigRational::one())).collect();
    let expected = oracle(&formula, &vtree, &w);
    assert_eq!(expected, BigRational::from_integer(30.into()), "the oracle drifted");
    let actual = marginalizing(&formula, &vtree, &w);
    assert_eq!(actual, expected, "lost model mass: deficit {}", &expected - &actual);
}

// ── Summing some variables ───────────────────────────────────────────────────

/// 4 variables, satisfiable, each in a clause.
fn leaf_formula() -> CnfFormula {
    make_formula(4, vec![vec![1, 2], vec![-2, 3], vec![3, -4]])
}

/// The weights a sum-out has to survive: fractional w⁺ ≠ w⁻, a negative
/// weight, a zero weight, w⁺ + w⁻ = 0, and w⁺ = w⁻, w⁻ = 0 and w⁺ = 0 side
/// by side.
fn weight_cases() -> Vec<(&'static str, Weights)> {
    vec![
        ("fractional", vec![(rat(1, 2), rat(1, 3)), (rat(2, 5), rat(3, 7)), (rat(5, 11), rat(2, 9)), (rat(1, 1), rat(4, 9))]),
        ("negative", vec![(rat(-3, 4), rat(1, 3)), (rat(2, 5), rat(-7, 2)), (rat(5, 11), rat(2, 9)), (rat(1, 1), rat(4, 9))]),
        ("zero", vec![(rat(0, 1), rat(1, 3)), (rat(2, 5), rat(0, 1)), (rat(5, 11), rat(2, 9)), (rat(1, 1), rat(4, 9))]),
        ("cancelling", vec![(rat(-1, 3), rat(1, 3)), (rat(2, 5), rat(3, 7)), (rat(-5, 11), rat(5, 11)), (rat(1, 1), rat(4, 9))]),
        ("all-equal", vec![(rat(1, 2), rat(1, 2)), (rat(3, 1), rat(3, 1)), (rat(2, 7), rat(2, 7)), (rat(5, 1), rat(5, 1))]),
        // x1 equal, x2 asymmetric, x3 w⁻ = 0, x4 w⁺ = 0.
        ("mixed-canon", vec![(rat(1, 2), rat(1, 2)), (rat(2, 5), rat(3, 7)), (rat(0, 1), rat(3, 1)), (rat(4, 1), rat(0, 1))]),
    ]
}

/// Summing out one variable, a pair, three or all four leaves the weighted
/// value as it is, and a second sum-out over a diagram already summed in part
/// reads what the first stored.
#[test]
fn any_set_of_summed_variables_keeps_the_value() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    let sets: [&[u32]; 8] = [&[1], &[2], &[3], &[4], &[1, 2], &[3, 4], &[1, 3, 4], &[1, 2, 3, 4]];
    for (label, w) in weight_cases() {
        let expected = oracle(&leaf_formula(), &vtree, &w);
        for set in sets {
            let mut tdd = plain_compile(eng, &leaf_formula(), &vtree);
            marginalize_weighted(eng, &mut tdd, &w, |v| set.contains(&v.0));
            assert_eq!(weighted_value(&tdd), expected, "[{label}] summing {set:?}");
            eng.marginalize(&mut tdd, |_| true).expect("no wall is armed");
            assert_eq!(weighted_value(&tdd), expected, "[{label}] summing {set:?}, then the rest");
        }
    }
}

/// With w⁺ = w⁻ everywhere the weighted compile keeps the value. Weights
/// differ across variables, so a read of the wrong polarity changes it.
#[test]
fn equal_weights_keep_the_value() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    let w: Weights = (1..=4).map(|i| (rat(i, 3), rat(i, 3))).collect();
    let expected = oracle(&leaf_formula(), &vtree, &w);
    let tdd = weighted_mc_compile(eng, &leaf_formula(), &vtree, &w);
    assert_eq!(weighted_value(&tdd), expected, "weighted compile");
}

/// XOR-shaped pairs leave duplicate `(α, leaf)` runs above the leaf
/// boundary. Under equal weights `2·Pos = One`, a value the column already
/// holds, so the duplicate-pair fold can collapse them. Whether it fires is a
/// size question; the value must hold either way. Under unit weights the
/// value is the model count.
#[test]
fn equal_weight_duplicate_runs_keep_the_value() {
    let eng = &Engine::new();
    let formula = make_formula(6, vec![vec![1, 2], vec![-1, -2], vec![3, 4], vec![-3, -4], vec![5, 6], vec![2, -4, 6]]);
    let vtree = Arc::new(Vtree::balanced(6));
    let models = compile_cnf(eng, &formula, &vtree, CompileOptions::default()).unwrap().diagram.model_count().unwrap();
    for (label, w) in [
        ("unit", (0..6).map(|_| (rat(1, 1), rat(1, 1))).collect::<Weights>()),
        ("equal-fractional", (1..=6).map(|i| (rat(i, 4), rat(i, 4))).collect()),
    ] {
        let expected = oracle(&formula, &vtree, &w);
        if label == "unit" {
            assert_eq!(expected, BigRational::from_integer(models.clone().into()));
        }
        let tdd = weighted_mc_compile(eng, &formula, &vtree, &w);
        assert_eq!(weighted_value(&tdd), expected, "[{label}]");
    }
}

// ── Fuzz ─────────────────────────────────────────────────────────────────────

/// The number `key` holds, or `default` when it is unset.
fn env_u32(key: &str, default: u32) -> u32 {
    match std::env::var(key) {
        Ok(value) => value.parse().unwrap_or_else(|_| panic!("`{key}={value}` is not a whole number")),
        Err(_) => default,
    }
}

/// Deterministic weights with zeros and negatives.
fn spread_weights(num_vars: u32) -> Weights {
    (0..num_vars as i64)
        .map(|v| {
            let w_neg = if v % 5 == 4 { BigRational::zero() } else { rat(v % 7 + 1, v + 3) };
            let sign = if v % 7 == 6 { -1 } else { 1 };
            (w_neg, rat(sign * (v % 3 + 1), v % 11 + 2))
        })
        .collect()
}

/// One of 0.1, 0.2, …, 0.9, drawn uniformly.
fn tenths(rng: &mut Lcg) -> BigRational {
    rat(rng.below(9) as i64 + 1, 10)
}

/// Weights shaped like real fractional instances, mostly equal pairs, with
/// every canonicalization edge: zero on one side, complements, asymmetric
/// pairs.
fn equalish_weights(num_vars: u32, seed: u64) -> Weights {
    let mut rng = Lcg::new(seed ^ 0xE9A1);
    (0..num_vars)
        .map(|_| {
            let w = tenths(&mut rng);
            match rng.below(20) {
                0 => (BigRational::zero(), w),
                1 => (w, BigRational::zero()),
                2 => (BigRational::one() - &w, w),
                3 => (tenths(&mut rng), w),
                _ => (w.clone(), w),
            }
        })
        .collect()
}

/// Plain positive fractions.
fn posfrac_weights(num_vars: u32, seed: u64) -> Weights {
    let mut rng = Lcg::new(seed ^ 0x5EED);
    (0..num_vars).map(|_| (tenths(&mut rng), tenths(&mut rng))).collect()
}

fn dump(formula: &CnfFormula, w: &[(BigRational, BigRational)]) -> String {
    let mut s = format!("num_vars {}\nclauses:\n", formula.num_vars());
    for c in formula.clauses() {
        let lits: Vec<String> = c.literals.iter().map(|l| l.to_dimacs().to_string()).collect();
        s.push_str(&format!("  {}\n", lits.join(" ")));
    }
    s.push_str("weights (w_neg, w_pos):\n");
    for (i, (n, p)) in w.iter().enumerate() {
        s.push_str(&format!("  x{}: ({n}, {p})\n", i + 1));
    }
    s
}

#[test]
#[ignore = "search harness; drive it with TIDIDI_FUZZ_* env vars"]
fn fuzz_weighted_marginalize_vs_oracle() {
    let (lo, hi) = (env_u32("TIDIDI_FUZZ_SEED_LO", 0), env_u32("TIDIDI_FUZZ_SEED_HI", 400));
    let (nv_lo, nv_hi) = (env_u32("TIDIDI_FUZZ_NVARS_LO", 6), env_u32("TIDIDI_FUZZ_NVARS_HI", 18));
    let (d_lo, d_hi) = (env_u32("TIDIDI_FUZZ_DENS_LO", 150), env_u32("TIDIDI_FUZZ_DENS_HI", 400));
    let stop = env_u32("TIDIDI_FUZZ_STOP", 1);
    let mode = std::env::var("TIDIDI_FUZZ_WEIGHTS").unwrap_or_else(|_| "spread".to_string());
    let weights_for: fn(u32, u64) -> Weights = match mode.as_str() {
        "spread" => |num_vars, _| spread_weights(num_vars),
        "posfrac" => posfrac_weights,
        "equalish" => equalish_weights,
        other => panic!("`TIDIDI_FUZZ_WEIGHTS={other}` is none of spread, posfrac, equalish"),
    };

    let (mut bad, mut checked) = (0u32, 0u32);
    for seed in lo..hi {
        let mut rng = Lcg::new(seed as u64 ^ 0xA11CE);
        let nv = nv_lo + rng.below(u64::from(nv_hi - nv_lo + 1)) as u32;
        let density = d_lo + rng.below(u64::from(d_hi - d_lo + 1)) as u32;
        let nc = ((nv * density) as f64 / 100.0).round().max(1.0) as usize;
        let formula = random_3cnf(nv, nc, seed as u64);
        let w = weights_for(nv, seed as u64);
        let vtree = build_vtree(&formula, "minfill-primal", &VtreeOptions::default()).unwrap();
        let want = oracle(&formula, &vtree, &w);
        let got = marginalizing(&formula, &vtree, &w);
        checked += 1;
        if got != want {
            bad += 1;
            println!("MISMATCH seed={seed} nv={nv} nc={nc}\n  want={want}\n  got ={got}\n{}", dump(&formula, &w));
            if bad >= stop {
                break;
            }
        }
    }
    println!("FUZZ DONE checked={checked} mismatches={bad}");
    assert_eq!(bad, 0, "weighted count differs from the full-diagram oracle");
}
