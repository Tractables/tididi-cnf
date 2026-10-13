//! The cofactor step against the normal step on the same children, and
//! counts through it against brute force. The fixtures are vtrees
//! `((S, B), top)`: the node under test `t` joins a small side `S` to a big
//! side `B`, and the root adds one more variable.

use super::*;
use std::cell::Cell;

use num_bigint::BigUint;
use num_rational::BigRational;
use tididi::diagram::{Arithmetic, WeightStore};
use vitri::cnf::CnfFormula;

use crate::compile::component::{CompileJob, PassOutput};
use crate::compile::marginalize::run_marginalize_at;
use crate::compile::schedule::marginalize_schedule;
use crate::compile::scope::group_clauses_by_scope;
use crate::compile::step::splice_and_fold;
use crate::compile::{CompileOptions, ComponentPlan, Output, WeightedCount, compile_cnf, weighted_count};
use crate::test_support::{self as fixtures, clause};

/// Clauses at `t` in each fixture: at least [`COFACTOR_MIN_CLAUSES`], and
/// enough that the normal fold takes three merges at the small floor, more
/// than the two branches of one small-side variable and fewer than the eight
/// of three.
fn crossing() -> usize {
    let floor = batch_floor(None).size;
    let crossing = COFACTOR_MIN_CLAUSES.max(2 * floor + 8);
    assert!(crossing.div_ceil(floor) == 3, "the fixtures' normal fold takes three merges");
    crossing
}

/// `n` clauses scoped at the node joining `s` to `b`: one literal over `s`
/// and three over `b`, every `b` variable in one of them, each satisfied
/// by the planted assignment that makes the odd variables true.
fn crossing_clauses(s: &[i32], b: &[i32], n: usize, seed: u64) -> Vec<Vec<i32>> {
    let mut rng = tididi::test_helpers::Lcg::new(seed);
    let planted = |lit: i32| (lit > 0) == (lit.abs() % 2 == 1);
    (0..n)
        .map(|i| {
            let mut vars = vec![s[i % s.len()], b[i % b.len()]];
            while vars.len() < 4 {
                let v = b[rng.below(b.len() as u64) as usize];
                if !vars.contains(&v) {
                    vars.push(v);
                }
            }
            let mut lits: Vec<i32> = vars.into_iter().map(|v| if rng.coin() { v } else { -v }).collect();
            if !lits.iter().any(|&l| planted(l)) {
                lits[1] = -lits[1];
            }
            lits
        })
        .collect()
}

struct Fixture {
    formula: CnfFormula,
    vtree: Arc<Vtree>,
    clauses: DiagramClauses,
    clauses_at: Vec<Vec<usize>>,
    /// The node under test: the root's left child, joining `S` to `B`.
    t: VtreeIdx,
    prune_spent_us: Cell<u64>,
}

impl Fixture {
    /// The vtree `((balanced S, balanced B), top)` and the DIMACS `clauses`.
    fn new(s: &[u32], b: std::ops::RangeInclusive<u32>, top: u32, clauses: Vec<Vec<i32>>) -> Fixture {
        let side = |vars: Vec<u32>| Vtree::balanced_over(&vars.into_iter().map(VarId).collect::<Vec<_>>()).unwrap();
        let t_tree = Vtree::join(&side(s.to_vec()), &side(b.collect())).unwrap();
        let vtree = Arc::new(Vtree::join(&t_tree, &Vtree::leaf(VarId(top))).unwrap());
        let formula = fixtures::formula(top, clauses.iter().map(|c| clause(c)).collect());
        let clauses_at = group_clauses_by_scope(&formula, &vtree);
        let t = vtree.children(vtree.root()).0;
        Fixture {
            clauses: DiagramClauses::new(&formula),
            formula,
            vtree,
            clauses_at,
            t,
            prune_spent_us: Cell::new(0),
        }
    }

    fn job<'a>(&'a self, output: PassOutput<'a>) -> CompileJob<'a> {
        CompileJob { deadline: None, output, run: Default::default(), prune_spent_us: &self.prune_spent_us }
    }

    fn schedule(&self) -> Vec<Vec<VtreeIdx>> {
        let lits: Vec<&[Literal]> = (0..self.formula.clauses().len()).map(|i| self.clauses.get(i)).collect();
        marginalize_schedule(&lits, &self.vtree, &self.clauses_at)
    }

    fn ctx<'a>(&'a self, job: CompileJob<'a>, schedule: Option<&'a [Vec<VtreeIdx>]>) -> StepContext<'a> {
        StepContext { job, clauses: &self.clauses, clauses_at: &self.clauses_at, marginalize_schedule: schedule }
    }

    /// Whether `node` lies in the subtree under `root`.
    fn under(&self, mut node: VtreeIdx, root: VtreeIdx) -> bool {
        loop {
            if node == root {
                return true;
            }
            match self.vtree.node(node).parent() {
                Some(parent) => node = parent,
                None => return false,
            }
        }
    }

    /// The conjunction of the clauses scoped under `root`, `None` when
    /// there are none: the diagram a pass hands up from `root`.
    fn conjoined_under(&self, eng: &Engine, root: VtreeIdx) -> Option<Tdd> {
        let mut acc: Option<Tdd> = None;
        for (scope, list) in self.clauses_at.iter().enumerate() {
            if !self.under(VtreeIdx(scope as u32), root) {
                continue;
            }
            for &i in list {
                let c = eng.clause(&self.vtree, self.clauses.get(i).iter().copied()).unwrap();
                acc = Some(match acc {
                    Some(a) => eng.and(a, c).unwrap(),
                    None => c,
                });
            }
        }
        acc
    }

    /// The slots a pass holds when it reaches `t`: each child's diagram,
    /// with the store of `output`'s weights.
    fn children(&self, eng: &Engine, output: PassOutput<'_>) -> Vec<Option<Tdd>> {
        let mut tdds = vec![None; self.vtree.num_nodes()];
        let (left, right) = self.vtree.children(self.t);
        for side in [left, right] {
            tdds[side.idx()] = self.conjoined_under(eng, side).map(|d| output.with_store(d));
        }
        tdds
    }

    /// The node's diagram by the cofactor step, `None` when it declines
    /// (which must leave the children in place), and by the normal step,
    /// each from the same children.
    fn both_steps(&self, eng: &Engine, ctx: &StepContext<'_>) -> (Option<Tdd>, Tdd) {
        let t = self.t;
        let mut tdds = self.children(eng, ctx.job.output);
        let held: Vec<bool> = tdds.iter().map(Option::is_some).collect();
        let cofactored = cofactor_step(eng, t, ctx, &mut tdds, &mut PassState::default(), &self.vtree)
            .unwrap()
            .map(|pairs| {
                let tdd = tdds[t.idx()].take().expect("a step taken stores the node's diagram");
                assert_eq!(pairs, tdd.pair_count(), "the step reports its diagram's pairs");
                tdd
            });
        if cofactored.is_none() {
            assert_eq!(tdds.iter().map(Option::is_some).collect::<Vec<_>>(), held, "a declined step leaves the slots");
        }
        let mut tdds = self.children(eng, ctx.job.output);
        splice_and_fold(eng, t, ctx, &mut tdds, &mut PassState::default(), &self.vtree).unwrap();
        (cofactored, tdds[t.idx()].take().expect("the normal step stores the node's diagram"))
    }

    /// The rest of a pass from `t`'s children: the step at `t` (the
    /// cofactor step, asserted to fire, or the normal one), its scheduled
    /// marginalization, then the root step and its own.
    fn pass(&self, eng: &Engine, ctx: &StepContext<'_>, cofactor: bool) -> Tdd {
        let (t, root) = (self.t, self.vtree.root());
        let mut tdds = self.children(eng, ctx.job.output);
        let state = &mut PassState::default();
        if cofactor {
            let taken = cofactor_step(eng, t, ctx, &mut tdds, state, &self.vtree).unwrap();
            assert!(taken.is_some(), "the cofactor step fires at t");
        } else {
            splice_and_fold(eng, t, ctx, &mut tdds, state, &self.vtree).unwrap();
        }
        run_marginalize_at(eng, t, ctx, &mut state.forgotten, &mut tdds, &self.vtree).unwrap();
        splice_and_fold(eng, root, ctx, &mut tdds, state, &self.vtree).unwrap();
        run_marginalize_at(eng, root, ctx, &mut state.forgotten, &mut tdds, &self.vtree).unwrap();
        tdds[root.idx()].take().expect("the root step stores the diagram")
    }

    /// `(negative, positive)` weights for every variable, none of them 1.
    fn weights(&self) -> Vec<(BigRational, BigRational)> {
        (1..=i64::from(self.formula.num_vars()))
            .map(|v| (BigRational::new((v + 1).into(), 3.into()), BigRational::new(2.into(), (v + 2).into())))
            .collect()
    }

    fn weight_store(&self) -> WeightStore {
        WeightStore::new(crate::convert::rational_weight_algebra(&self.weights()), Arithmetic::ExactRational)
    }

    /// The formula's model count, by brute force.
    fn brute_count(&self) -> BigUint {
        fixtures::brute_projected_count(&self.formula, 0..self.formula.num_vars() as usize)
    }

    /// The formula's weighted count, by brute force.
    fn brute_weighted_count(&self) -> BigRational {
        fixtures::brute_projected_weighted_count(&self.formula, 0..self.formula.num_vars() as usize, &self.weights())
    }
}

/// One small-side variable (`x1`, with no diagram of its own), eleven on
/// the big side, `x13` at the root; `extra` crossing clauses added.
fn one_variable_fixture(extra: &[&[i32]]) -> Fixture {
    let b: Vec<i32> = (2..=12).collect();
    let mut clauses = crossing_clauses(&[1], &b, crossing(), 0x9e37_79b9_7f4a_7c15);
    clauses.extend(extra.iter().map(|c| c.to_vec()));
    // The big side's own clauses, and two at the root that keep x2, x5 and
    // x9 explicit past t while the rest of t's levels finish there.
    clauses.extend([vec![2, 3], vec![-4, 5, 6], vec![13, 2], vec![-13, 5, -9]]);
    let fx = Fixture::new(&[1], 2..=12, 13, clauses);
    assert!(fx.clauses_at[fx.t.idx()].len() >= crossing());
    fx
}

/// Three small-side variables `x1..x3`, ten on the big side, `x14` at the
/// root; with `restricted`, the small side's own clauses allow two of its
/// eight assignments.
fn three_variable_fixture(restricted: bool) -> Fixture {
    let b: Vec<i32> = (4..=13).collect();
    let mut clauses = crossing_clauses(&[1, 2, 3], &b, crossing(), 0x2545_f491_4f6c_dd1d);
    if restricted {
        clauses.extend([vec![1], vec![2, 3], vec![-2, -3]]);
    }
    clauses.extend([vec![4, 5], vec![-6, 7, 8], vec![14, 4], vec![-14, 7, -11]]);
    Fixture::new(&[1, 2, 3], 4..=13, 14, clauses)
}

#[test]
fn the_cofactor_step_builds_the_normal_steps_function() {
    let eng = &Engine::new();
    let fx = one_variable_fixture(&[]);
    let ctx = fx.ctx(fx.job(PassOutput::Function), None);
    let (cofactored, normal) = fx.both_steps(eng, &ctx);
    let cofactored = cofactored.expect("one small-side variable and enough clauses: the step fires");
    assert!(eng.equivalent(&cofactored, &normal).unwrap());
    let reference = fx.conjoined_under(eng, fx.t).unwrap();
    assert!(eng.equivalent(&cofactored, &reference).unwrap());
    assert!(eng.is_sat(&cofactored).unwrap(), "the planted assignment is a model");
}

/// Under x1 = false the two added clauses leave x2 and ¬x2: that branch
/// folds to ⊥ and is dropped, and the step still builds the function.
#[test]
fn the_cofactor_step_drops_an_unsatisfiable_branch() {
    let eng = &Engine::new();
    let fx = one_variable_fixture(&[&[1, 2], &[1, -2]]);
    let ctx = fx.ctx(fx.job(PassOutput::Function), None);
    let (cofactored, normal) = fx.both_steps(eng, &ctx);
    let cofactored = cofactored.expect("the step fires");
    assert!(eng.equivalent(&cofactored, &normal).unwrap());
    assert!(eng.is_sat(&cofactored).unwrap());
    let x1_false = eng.condition(cofactored, [Literal::neg(VarId(1))]).unwrap();
    assert!(!eng.is_sat(&x1_false).unwrap(), "the x1 = false branch is gone");
}

#[test]
fn the_cofactor_step_branches_on_what_the_small_side_allows() {
    let eng = &Engine::new();
    let fx = three_variable_fixture(true);
    let ctx = fx.ctx(fx.job(PassOutput::Function), None);
    let (cofactored, normal) = fx.both_steps(eng, &ctx);
    let cofactored = cofactored.expect("two allowed assignments, three merges: the step fires");
    assert!(eng.equivalent(&cofactored, &normal).unwrap());
    assert!(eng.equivalent(&cofactored, &fx.conjoined_under(eng, fx.t).unwrap()).unwrap());
}

/// Unrestricted, the three variables give eight branches, more than the
/// normal fold's three merges.
#[test]
fn the_cofactor_guard_declines_more_branches_than_merges() {
    let eng = &Engine::new();
    let fx = three_variable_fixture(false);
    let ctx = fx.ctx(fx.job(PassOutput::Function), None);
    let (cofactored, _) = fx.both_steps(eng, &ctx);
    assert!(cofactored.is_none(), "eight branches against three merges: the guard declines");
}

#[test]
fn the_cofactor_step_declines_a_child_with_a_marginal_level() {
    let eng = &Engine::new();
    let fx = one_variable_fixture(&[]);
    let ctx = fx.ctx(fx.job(PassOutput::Function), None);
    let big = fx.vtree.children(fx.t).1;
    let mut tdds = fx.children(eng, ctx.job.output);
    // A child index precedes its parent's, so ascending is bottom-up.
    let levels: Vec<VtreeIdx> =
        (0..fx.vtree.num_nodes() as u32).map(VtreeIdx).filter(|&u| fx.under(u, big)).collect();
    eng.marginalize_levels(tdds[big.idx()].as_mut().unwrap(), &levels).unwrap();
    assert!(tdds[big.idx()].as_ref().unwrap().has_marginal_level());
    let taken = cofactor_step(eng, fx.t, &ctx, &mut tdds, &mut PassState::default(), &fx.vtree).unwrap();
    assert!(taken.is_none(), "a marginal child is left to the normal step");
    assert!(tdds[big.idx()].is_some() && tdds[fx.t.idx()].is_none());
}

#[test]
fn the_cofactor_step_counts_under_the_marginalize_schedule() {
    let eng = &Engine::new();
    for fx in [one_variable_fixture(&[]), one_variable_fixture(&[&[1, 2], &[1, -2]]), three_variable_fixture(true)] {
        let schedule = fx.schedule();
        assert!(
            !schedule[fx.t.idx()].is_empty() && !schedule[fx.vtree.root().idx()].is_empty(),
            "levels finish at t and at the root"
        );
        let ctx = fx.ctx(fx.job(PassOutput::Count(None)), Some(schedule.as_slice()));
        let expected = fx.brute_count();
        assert!(expected > BigUint::ZERO, "the planted assignment is a model");
        assert_eq!(fx.pass(eng, &ctx, true).model_count().unwrap(), expected);
        assert_eq!(fx.pass(eng, &ctx, false).model_count().unwrap(), expected);
    }
}

/// A weight store holds values only at marginal levels, so the branches,
/// over children with none, carry no values: the step-end marginalization
/// weighs the cofactor step's diagram as it weighs the normal step's.
#[test]
fn the_cofactor_step_weighs_as_the_normal_step() {
    let eng = &Engine::new();
    for fx in [one_variable_fixture(&[]), three_variable_fixture(true)] {
        let schedule = fx.schedule();
        let store = fx.weight_store();
        let ctx = fx.ctx(fx.job(PassOutput::Count(Some(&store))), Some(schedule.as_slice()));
        let value =
            |tdd: Tdd| eng.weighted_value(&tdd).unwrap().expect("a weighted diagram").as_rational().into_owned();
        let expected = fx.brute_weighted_count();
        assert_eq!(value(fx.pass(eng, &ctx, true)), expected);
        assert_eq!(value(fx.pass(eng, &ctx, false)), expected);
    }
}

/// The fixtures through the whole compile, which takes the step at t (the
/// tests above show it fires on these children): the Boolean, counting and
/// weighted answers are the formula's.
#[test]
fn compiles_through_the_cofactor_step_answer_exactly() {
    let eng = &Engine::new();
    for fx in [one_variable_fixture(&[]), one_variable_fixture(&[&[1, 2], &[1, -2]]), three_variable_fixture(true)] {
        let expected = fx.brute_count();
        for output in [Output::Function, Output::Count] {
            let opts = CompileOptions { plan: ComponentPlan::Whole, output, deadline: None };
            let tdd = compile_cnf(eng, &fx.formula, &fx.vtree, opts).unwrap().diagram;
            assert_eq!(tdd.model_count().unwrap(), expected);
        }
        let algebra = crate::convert::rational_weight_algebra(&fx.weights());
        let weighed =
            weighted_count(eng, &fx.formula, &fx.vtree, ComponentPlan::Whole, algebra, Arithmetic::ExactRational, None);
        let Some(WeightedCount::Satisfiable(value)) = weighed else {
            panic!("a satisfiable fixture weighs");
        };
        assert_eq!(*value.as_rational(), fx.brute_weighted_count());
    }
}
