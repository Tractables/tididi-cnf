//! Weighted marginalization against the full-diagram oracle: a plain
//! `compile_cnf` evaluated under the same weights, which shares no
//! marginal-store code with it. Covers the weighted marginalizing compile
//! (`Output::WeightedCount`, the compile the binary's weighted count runs),
//! weighted marginalization of a plain compile, and the weight-marginal
//! vtree leaf: its column, the diagrams that share it, and the equal-weight
//! ref canonicalization.
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
use tididi::diagram::{Arithmetic, Tdd, WeightStore};
use tididi::test_helpers::{Lcg, exact_weight, rat};
use tididi::vtree::{VarId, Vtree, VtreeIdx};
use tididi_bonsai::compile::{CompileOptions, Output, compile_cnf};
use tididi_bonsai::vtree::{VtreeOptions, build_vtree};

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

/// The oracle: a plain compile evaluated under `w`.
fn oracle(formula: &CnfFormula, vtree: &Arc<Vtree>, w: &[(BigRational, BigRational)]) -> BigRational {
    compile_cnf(&Engine::new(), formula, vtree, CompileOptions::default())
        .unwrap()
        .diagram
        .evaluate(&rational_weight_algebra(w))
        .unwrap()
}

/// The marginalizing compile's count. Its verdict must be the formula's,
/// whatever the weights: unsatisfiable exactly when a plain compile has no
/// model, and satisfiable when the count is zero only through its weights.
fn marginalizing(formula: &CnfFormula, vtree: &Arc<Vtree>, w: &[(BigRational, BigRational)]) -> BigRational {
    let tdd = weighted_mc_compile(&Engine::new(), formula, vtree, w);
    let satisfiable = compile_cnf(&Engine::new(), formula, vtree, CompileOptions::default()).unwrap().diagram.is_sat().unwrap();
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

/// Attach `w` to `tdd` and marginalize `targets` in one batch.
fn marginalize_weighted(eng: &Engine, tdd: &mut Tdd, w: &[(BigRational, BigRational)], targets: &[VtreeIdx]) {
    tdd.set_weights(WeightStore::new(rational_weight_algebra(w), Arithmetic::ExactRational))
        .expect("the weights cover the diagram's live columns");
    eng.marginalize_levels(tdd, targets).expect("no wall is armed");
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
        let mut plain = compile_cnf(eng, &formula, &vtree, CompileOptions::default()).unwrap().diagram;
        if w.iter().all(|(neg, pos)| neg.is_one() && pos.is_one()) {
            assert_eq!(expected, BigRational::from_integer(plain.model_count().unwrap().into()), "unit weights count models");
        }
        assert_eq!(marginalizing(&formula, &vtree, &w), expected, "marginalizing compile, {clauses:?}");
        marginalize_weighted(eng, &mut plain, &w, vtree.bottomup_slice());
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

// ── Weight-marginal vtree leaves ─────────────────────────────────────────────
//
// A weight-marginal leaf holds a 3-slot column in `LeafLabel::from_idx` order
// (0 = One = w⁺ + w⁻, 1 = Pos = w⁺, 2 = Neg = w⁻). The column is a
// compile-wide cache keyed by vtree index and shared by every diagram of the
// compile, so no one diagram may compact, erase or reinterpret it.

/// 4 variables, satisfiable, each in a clause, so every leaf is a real
/// marginalize target.
fn leaf_formula() -> CnfFormula {
    make_formula(4, vec![vec![1, 2], vec![-2, 3], vec![3, -4]])
}

/// The weights a leaf column has to survive: fractional w⁺ ≠ w⁻, a negative
/// weight, a zero weight, w⁺ + w⁻ = 0 (an exactly zero One slot), and the
/// three shapes of the equal-value ref canonicalization: w⁺ = w⁻ (Neg → Pos),
/// w⁻ = 0 (Pos → One), w⁺ = 0 (Neg → One). "mixed-canon" puts all three and
/// an asymmetric variable in one compile, so a per-leaf map cannot pass as a
/// per-compile one; the rest must leave the canonicalization idle.
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

/// The leaf of variable `var` (zero-based) is weight-marginal and holds its
/// full column.
fn assert_leaf_column(tdd: &Tdd, vtree: &Vtree, var: u32, w: &(BigRational, BigRational), label: &str) {
    let leaf = vtree.leaf_of(VarId(var + 1)).expect("the vtree carries this variable");
    let level = &tdd.levels()[leaf.idx()];
    assert!(level.is_weight_marginal(), "[{label}] leaf of x{} is not weight-marginal", var + 1);
    assert_eq!(level.slot_count(), 3, "[{label}] leaf of x{}: the slot count must stay at `LEAF_WIDTH`", var + 1);
    let column = tdd
        .weights()
        .expect("weights attached")
        .level(leaf.idx())
        .unwrap_or_else(|| panic!("[{label}] leaf of x{} lost its column", var + 1));
    let (w_neg, w_pos) = w;
    assert_eq!(column.len(), 3, "[{label}] the leaf column carries One, Pos and Neg");
    assert_eq!(exact_weight(&column[0]), w_pos + w_neg, "[{label}] slot 0 is One = w⁺ + w⁻");
    assert_eq!(exact_weight(&column[1]), w_pos.clone(), "[{label}] slot 1 is Pos = w⁺");
    assert_eq!(exact_weight(&column[2]), w_neg.clone(), "[{label}] slot 2 is Neg = w⁻");
}

/// A lone leaf target turns weight-marginal and leaves the value alone.
#[test]
fn a_leaf_target_turns_weight_marginal() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    for (label, w) in weight_cases() {
        let expected = oracle(&leaf_formula(), &vtree, &w);
        for var in 0..4u32 {
            let mut tdd = compile_cnf(eng, &leaf_formula(), &vtree, CompileOptions::default()).unwrap().diagram;
            let leaf = vtree.leaf_of(VarId(var + 1)).expect("the vtree carries this variable");
            marginalize_weighted(eng, &mut tdd, &w, &[leaf]);
            assert_leaf_column(&tdd, &vtree, var, &w[var as usize], label);
            assert_eq!(weighted_value(&tdd), expected, "[{label}] marginalizing the leaf of x{}", var + 1);
        }
    }
}

/// One batch with an internal target and a leaf outside its subtree: the
/// internal one takes the main cascade, the leaf the closing leaf pass.
#[test]
fn a_batch_mixes_leaf_and_internal_targets() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    let leaf = vtree.leaf_of(VarId(1)).expect("the vtree carries this variable");
    let (a, b) = vtree.children(vtree.root());
    let mut climb = vtree.node(leaf).parent();
    while let Some(p) = climb.filter(|&p| p != a && p != b) {
        climb = vtree.node(p).parent();
    }
    let internal = if climb == Some(a) { b } else { a };
    assert!(!vtree.node(internal).is_leaf(), "the other root child of balanced(4) is internal");

    for (label, w) in weight_cases() {
        let expected = oracle(&leaf_formula(), &vtree, &w);
        let mut tdd = compile_cnf(eng, &leaf_formula(), &vtree, CompileOptions::default()).unwrap().diagram;
        marginalize_weighted(eng, &mut tdd, &w, &[internal, leaf]);
        assert!(tdd.levels()[internal.idx()].is_weight_marginal(), "[{label}] internal target");
        assert_leaf_column(&tdd, &vtree, 0, &w[0], label);
        assert_eq!(weighted_value(&tdd), expected, "[{label}]");
    }
}

/// Marginalizing only the leaves (each parent stays structural, so each
/// column is a boundary store the slot prune walks) and marginalizing
/// everything (each leaf under a marginal parent, the prune's dead-store
/// walk) both keep every leaf's full column. A column compacted to the slots
/// one diagram's parent uses would break every other diagram reading it.
#[test]
fn leaf_columns_survive_the_slot_prune() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    for (label, w) in weight_cases() {
        let expected = oracle(&leaf_formula(), &vtree, &w);
        for (phase, targets) in [
            ("leaves", vtree.leaf_bottomup().map(|(t, _)| t).collect::<Vec<_>>()),
            ("everything", vtree.bottomup_slice().to_vec()),
        ] {
            let mut tdd = compile_cnf(eng, &leaf_formula(), &vtree, CompileOptions::default()).unwrap().diagram;
            marginalize_weighted(eng, &mut tdd, &w, &targets);
            for var in 0..4u32 {
                assert_leaf_column(&tdd, &vtree, var, &w[var as usize], &format!("{label}/{phase}"));
            }
            let subsumed = tididi::test_helpers::check::marginal::subsumed_marginal_data_violations(&tdd);
            assert!(subsumed.is_empty(), "[{label}/{phase}] data left under a marginal parent at {subsumed:?}");
            assert_eq!(weighted_value(&tdd), expected, "[{label}/{phase}]");
        }
    }
}

/// Diagram A marginalizes an internal node and a leaf under it; diagram B
/// takes A's store (as a conjunction of the two would) but keeps that leaf
/// structural, so B's parent refs to it are leaf labels. They must resolve by
/// label, not through the column, and a subsumed leaf keeps its full column,
/// so B neither reads out of bounds nor drops the leaf's mass.
#[test]
fn a_structural_leaf_reads_by_label_under_a_shared_store() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    let parent = (0..vtree.num_nodes() as u32)
        .map(VtreeIdx)
        .find(|&p| {
            !vtree.node(p).is_leaf() && {
                let (l, r) = vtree.children(p);
                vtree.node(l).is_leaf() && vtree.node(r).is_leaf()
            }
        })
        .expect("balanced(4) has an internal node over two leaves");
    let (leaf, _) = vtree.children(parent);

    for (label, w) in weight_cases() {
        let mut a = compile_cnf(eng, &leaf_formula(), &vtree, CompileOptions::default()).unwrap().diagram;
        marginalize_weighted(eng, &mut a, &w, &[parent, leaf]);
        let shared = a.weights().expect("weights attached").clone();
        assert!(
            shared.level(leaf.idx()).is_some_and(|column| column.len() == 3),
            "[{label}] a subsumed leaf keeps its column"
        );

        let mut b = compile_cnf(eng, &leaf_formula(), &vtree, CompileOptions::default()).unwrap().diagram;
        let expected = b.evaluate(&rational_weight_algebra(&w)).unwrap();
        b.set_weights(shared).expect("the weights cover the diagram's live columns");
        eng.marginalize_levels(&mut b, &[parent]).expect("no wall is armed");
        assert_eq!(weighted_value(&b), expected, "[{label}]");
    }
}

/// Through the weighted marginalizing compile, where conjunction, pair
/// fusion and the slot prune all act on leaf boundaries, some leaf ends
/// weight-marginal and the value matches the oracle.
#[test]
fn the_weighted_compile_marginalizes_leaves() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    for (label, w) in weight_cases() {
        let tdd = weighted_mc_compile(eng, &leaf_formula(), &vtree, &w);
        let marginal_leaves =
            tdd.vtree().leaf_bottomup().filter(|(t, _)| tdd.levels()[t.idx()].is_marginal()).count();
        assert!(marginal_leaves > 0, "[{label}] no leaf level was marginalized");
        assert_eq!(weighted_value(&tdd), oracle(&leaf_formula(), &vtree, &w), "[{label}]");
    }
}

/// Counts the parent refs into weight-marginal leaves under structural
/// parents, asserting none names the Neg slot (raw 2). With w⁺ = w⁻ at every
/// variable the equal-value map sends Neg to Pos, so a canonical ref never
/// names slot 2.
fn count_canonical_leaf_refs(tdd: &Tdd, label: &str) -> usize {
    let vtree = tdd.vtree();
    let mut seen = 0;
    for (leaf, _) in vtree.leaf_bottomup() {
        let Some(parent) = vtree.node(leaf).parent() else { continue };
        if !tdd.levels()[leaf.idx()].is_weight_marginal() || tdd.levels()[parent.idx()].is_marginal() {
            continue;
        }
        let leaf_is_left = vtree.children(parent).0 == leaf;
        let level = &tdd.levels()[parent.idx()];
        for n in 0..level.nodes().len() {
            for pair in level.pairs_of_idx(n) {
                let raw = if leaf_is_left { pair.left.raw() } else { pair.right.raw() };
                assert_ne!(raw, 2, "[{label}] a Neg-slot ref into weight-marginal leaf {leaf:?} under equal weights");
                seen += 1;
            }
        }
    }
    seen
}

/// With w⁺ = w⁻ everywhere, no ref into a weight-marginal leaf names the Neg
/// slot after a leaves-only batch (where the leaf pass rewrites them) or
/// after the weighted compile (where conjunction and the duplicate-pair fold
/// create them), and the value is unchanged. Weights differ across variables,
/// so a wrong-slot read changes the value.
#[test]
fn equal_weights_leave_no_neg_slot_refs() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    let w: Weights = (1..=4).map(|i| (rat(i, 3), rat(i, 3))).collect();
    let expected = oracle(&leaf_formula(), &vtree, &w);

    let mut tdd = compile_cnf(eng, &leaf_formula(), &vtree, CompileOptions::default()).unwrap().diagram;
    let leaves: Vec<VtreeIdx> = vtree.leaf_bottomup().map(|(t, _)| t).collect();
    marginalize_weighted(eng, &mut tdd, &w, &leaves);
    assert!(count_canonical_leaf_refs(&tdd, "leaves") > 0, "the leaves-only batch left no ref to inspect");
    assert_eq!(weighted_value(&tdd), expected, "leaves-only batch");

    // A fully marginalized diagram may leave no such ref; any that is there
    // must be canonical.
    let tdd = weighted_mc_compile(eng, &leaf_formula(), &vtree, &w);
    count_canonical_leaf_refs(&tdd, "compile");
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
        count_canonical_leaf_refs(&tdd, label);
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
