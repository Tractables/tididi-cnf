//! Projected counting by enumerating the constrained show variables.
//!
//! The show vars that occur in some clause are the constrained set
//! ([`split_show`]); the rest are free, and scale a count by their factor.
//! Four routines work over the constrained set:
//!
//! - the 2^K sweep ([`sweep`]) assumption-solves every
//!   assignment to at most [`SWEEP_MAX_SHOW`] constrained vars:
//!   `pmc = 2^|free show| * #{ a : formula ∧ a SAT }`;
//! - the projected AllSAT enumeration ([`allsat`]) finds
//!   only the feasible assignments, one blocking clause each, so it reaches
//!   large constrained sets whose feasible count is small;
//! - the path estimator ([`estimate_by_paths`], drawing one [`random_path`]
//!   at a time) estimates that feasible count, so the
//!   support-enum stage can decide whether to enumerate and an enumeration
//!   under [`AllsatCtl::forecast`] can stop once the count it is heading for
//!   cannot fit its wall;
//! - the feasibility probe ([`feasible_fraction_estimate`]) samples random
//!   assignments, so PMC can skip enumerating a near-full projection.
//!
//! [`show_enum_fastpath`] is the entry the counts call: the sweep, then
//! AllSAT, under [`EnumLimits`].
//!
//! The sweep and the enumeration add up a [`Tally`]: [`Models`] counts the
//! feasible assignments, [`WeightedModels`] sums their weight for PWMC. CaDiCaL
//! only decides whether each show assignment extends to a model, with the
//! projected variables free in the solve; the counting is done here, so both
//! counts are exact. The two estimates only route.

use std::time::{Duration, Instant};

use vitri::sat::{Bounded, CaDiCal, Status};
use num_bigint::BigUint;
use num_rational::BigRational;
use num_traits::Zero;

use vitri::cnf::CnfFormula;
use tididi::vtree::VarId;

use crate::problem::WeightedCount;
use crate::problem::ShowSet;

/// Split the show set into the constrained vars (in range and occurring in
/// some clause, returned as DIMACS ids) and the free ones (absent from every
/// clause, so feasible in both polarities and never enumerated).
///
/// The one place this split is defined: every enumerator, and the path
/// estimator, must agree on it exactly, or a count and its estimate end up in
/// different units (the free-show factor would be applied to one and not the
/// other). [`show_enum_fastpath`] splits once and hands the split to each
/// enumerator it runs.
fn split_show(formula: &CnfFormula, show_vars: &[VarId]) -> ShowSplit {
    let occurrences = crate::formula::occurrence_counts(formula, 0);
    let (constrained, free): (Vec<VarId>, Vec<VarId>) = show_vars
        .iter()
        .copied()
        .partition(|v| occurrences.get(v.idx()).is_some_and(|&n| n > 0));
    ShowSplit { constrained: constrained.iter().map(|v| v.0 as i32).collect(), free }
}

/// A show set split against a formula ([`split_show`]).
struct ShowSplit {
    /// The show variables some clause mentions, as DIMACS ids.
    constrained: Vec<i32>,
    /// The show variables no clause mentions.
    free: Vec<VarId>,
}

/// How many variables of `show` [`split_show`] calls constrained.
pub(crate) fn constrained_show_count(formula: &CnfFormula, show: &ShowSet) -> usize {
    split_show(formula, &show_vars(show)).constrained.len()
}

/// The show set as the diagram's variables, ascending: the walk order of every
/// enumerator and of the estimator, and so the one order that belongs to the
/// formula rather than to the process.
pub(crate) fn show_vars(show: &ShowSet) -> Vec<VarId> {
    show.iter_vars().map(crate::convert::diagram_var).collect()
}

/// Largest constrained-show set the 2^K sweep enumerates.
const SWEEP_MAX_SHOW: usize = 16;

/// Wall the 2^K sweep may spend before it declines. Checked between solves.
const SWEEP_WALL: Duration = Duration::from_secs(60);

/// The AllSAT model cap for a caller that sets none of its own: past it the
/// enumeration stops and the count falls through to the compile.
const MODEL_CAP: u64 = 200_000;

/// A CaDiCaL solver loaded with `formula`, the `constrained` vars frozen so
/// inprocessing cannot eliminate the vars the caller will assume or read.
fn load_solver(formula: &CnfFormula, constrained: &[i32]) -> Option<CaDiCal> {
    let mut solver = CaDiCal::new()?;
    for cl in formula.clauses() {
        for lit in &cl.literals {
            solver.add(lit.to_dimacs());
        }
        solver.add(0);
    }
    for &d in constrained {
        solver.freeze(d);
    }
    Some(solver)
}

/// What an enumerator adds up over the feasible constrained-show
/// assignments it finds.
pub(crate) trait Tally {
    type Value;
    /// One feasible assignment, as the DIMACS literal of each constrained var.
    fn add(&mut self, assignment: &[i32]);
    /// The count: the tally scaled by the `free` show vars.
    fn total(self, free: &[VarId]) -> Self::Value;
}

/// Projected model count: the number of feasible assignments, times
/// 2^|free show|.
#[derive(Clone, Default)]
pub(crate) struct Models(u64);

impl Tally for Models {
    type Value = BigUint;
    fn add(&mut self, _assignment: &[i32]) {
        self.0 += 1;
    }
    fn total(self, free: &[VarId]) -> BigUint {
        BigUint::from(self.0) << free.len()
    }
}

/// Projected weighted count:
///
///   pwmc = [ Σ_{feasible σ over constrained show} ∏_{constr s} w(s, σ(s)) ]
///          × ∏_{free show s} (w_pos[s] + w_neg[s])
///
/// Projected vars are weight-1 and existentially free; a feasible σ
/// contributes the product of its show literals' weights. A free show var is
/// feasible in both polarities, so contributes (w_pos+w_neg), or 1 when it
/// has no weight entry. With all weights 1 this is [`Models`].
/// `weights` holds the `(w_neg, w_pos)` pair of each var, indexed by
/// `VarId::idx`.
///
/// The total is unsatisfiable when no assignment was feasible, which the sum
/// cannot say: a zero weight, or weights that cancel, sum a satisfiable
/// formula's projections to zero. Every enumerator tries the empty
/// assignment when nothing is constrained, so a satisfiable formula always
/// adds one.
#[derive(Clone)]
pub(crate) struct WeightedModels<'a> {
    weights: &'a [(BigRational, BigRational)],
    sum: BigRational,
    feasible: bool,
}

impl<'a> WeightedModels<'a> {
    pub(crate) fn new(weights: &'a [(BigRational, BigRational)]) -> Self {
        WeightedModels { weights, sum: BigRational::zero(), feasible: false }
    }
}

impl Tally for WeightedModels<'_> {
    type Value = WeightedCount<BigRational>;
    fn add(&mut self, assignment: &[i32]) {
        let prod: BigRational = assignment
            .iter()
            .map(|&lit| {
                let (w_neg, w_pos) = &self.weights[(lit.unsigned_abs() - 1) as usize];
                if lit > 0 { w_pos } else { w_neg }
            })
            .product();
        self.sum += prod;
        self.feasible = true;
    }
    fn total(self, free: &[VarId]) -> WeightedCount<BigRational> {
        if !self.feasible {
            return WeightedCount::Unsatisfiable;
        }
        let free_factor: BigRational = free
            .iter()
            .filter_map(|v| self.weights.get(v.idx()))
            .map(|(w_neg, w_pos)| w_pos + w_neg)
            .product();
        WeightedCount::Satisfiable(self.sum * free_factor)
    }
}

/// The 2^|constrained| sweep: compute the projected count by enumerating a
/// small constrained-show set, adding each feasible assignment to `tally`.
/// Returns `None` (caller falls through to the normal compile path) when the
/// constrained-show set exceeds [`SWEEP_MAX_SHOW`], when [`SWEEP_WALL`] or
/// `deadline`, the run's wall, runs out, or when the oracle returns unknown.
///
/// `split` is the show set's split against `formula`. Soundness: each SAT
/// result means the show assignment extends to a model (projected vars
/// existentially free); the tally of SAT assignments, scaled by the free show
/// vars, is exactly the projected count.
fn sweep<T: Tally>(
    formula: &CnfFormula,
    split: &ShowSplit,
    mut tally: T,
    deadline: Option<Instant>,
) -> Option<T::Value> {
    let ShowSplit { constrained, free } = split;

    if constrained.len() > SWEEP_MAX_SHOW {
        return None;
    }

    let start = Instant::now();
    let mut solver = load_solver(formula, constrained)?;

    let k = constrained.len();
    let total: u64 = 1u64 << k;
    let mut assignment: Vec<i32> = Vec::with_capacity(k);
    for mask in 0..total {
        if start.elapsed() > SWEEP_WALL || deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return None;
        }
        assignment.clear();
        for (bit, &d) in constrained.iter().enumerate() {
            let pos = (mask >> bit) & 1 == 1;
            let lit = if pos { d } else { -d };
            assignment.push(lit);
            solver.assume(lit);
        }
        match solver.solve() {
            Status::Satisfiable => tally.add(&assignment),
            Status::Unsatisfiable => {}
            Status::Unknown => return None,
        }
    }

    Some(tally.total(free))
}

/// Why a projected AllSAT enumeration returned `None` instead of a complete
/// feasible set. Reported through [`AllsatCtl::bust`] so a caller can print
/// the actual reason rather than an ambiguous "model or wall cap".
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AllsatStop {
    /// The wall budget was already spent when a solve was about to start.
    Wall,
    /// CaDiCaL returned unknown — under a wall callback that is the connected
    /// terminator cutting a solve that was still running at the deadline.
    Terminated,
    /// More feasible projections than `model_budget`.
    ModelCap,
    /// The tally passed the bound a [`WallGrant`] was made on, so the
    /// estimate that justified granting the wall is disproved by the
    /// enumeration itself. Distinct from `ModelCap`: the cap is a standing
    /// ceiling, this is a caller's own premise failing mid-run.
    TallyCeiling,
    /// The count forecast taken at [`AllsatCtl::forecast`] estimated this
    /// many feasible projections, more than [`FORECAST_MARGIN`] times the
    /// wall left holds at the rate the enumeration has run so far.
    Forecast(f64),
}

/// What a caller grants when an enumeration reaches its wall — more time,
/// plus the terms it was granted on.
///
/// Carrying the terms with the grant is what keeps them honest: a caller that
/// admitted the enumeration on a bound states, in the same breath, the tally
/// that would disprove that bound — so there is no second channel, and no way
/// to extend the wall while forgetting the terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WallGrant {
    /// Extra wall, measured from now.
    pub(crate) extra: Duration,
    /// Tally past which this grant's own justification no longer holds: once
    /// the running model count exceeds it, the enumeration stops with
    /// [`AllsatStop::TallyCeiling`].
    pub(crate) abort_at_models: u64,
}

/// Caller control and observation for a single projected-AllSAT enumeration.
///
/// `AllsatCtl::default()` is the plain enumerator: no callback, so the wall is
/// checked only between solves, and no forecast. The PMC and PWMC
/// enumerate-show stages run it with one addition, the count
/// [`forecast`](Self::forecast) at [`FORECAST_AT`]; the model count's
/// support-enum stage supervises through [`at_wall`](Self::at_wall) and its
/// own grant terms instead.
#[derive(Default)]
pub(crate) struct AllsatCtl<'a> {
    /// Consulted when the wall budget runs out (between solves, or when a
    /// solve is cut by the terminator). `Some(grant)` resumes the same
    /// solver — blocking-clause DB, learned clauses and model tally all
    /// intact — for `grant.extra` more wall, under `grant.abort_at_models`;
    /// `None` busts. Absent ⇒ always bust.
    ///
    /// A caller that supplies it, the model count's support-enum stage, is
    /// supervising the enumeration and gets a wall-clock terminator with it,
    /// which bounds each single solve, not just the gap between solves: an
    /// enumeration that is going to bust must never burn wall the recovery
    /// cascade is owed. PMC/PWMC supply none: they check only between
    /// solves, because cutting a solve they would have finished turns a
    /// complete count into a fall-through.
    pub(crate) at_wall: Option<&'a mut dyn FnMut(u64, Duration) -> Option<WallGrant>>,
    /// Which cap actually tripped, set on every `None` return that is a
    /// bust. Left as the caller set it when [`allsat`] returns `None` because
    /// the SAT solver could not be created.
    pub(crate) bust: Option<AllsatStop>,
    /// Feasible projections tallied when the enumeration returned, complete
    /// or not. On a bust this is a lower bound on the true count — which is
    /// exactly what a caller estimating the count from a truncated run wants.
    pub(crate) models: u64,
    /// When to forecast. `Some(at)` asks for one count forecast, taken at the
    /// first point between solves at least `at` into the enumeration with a
    /// model tallied, so that there is a rate to forecast from: the path
    /// estimator ([`estimate_by_paths`], at most [`FORECAST_PATHS`] paths
    /// under [`FORECAST_WALL`] or the wall left, whichever is less, all of it
    /// spent from the enumeration's own wall) estimates the feasible count N̂,
    /// and when the projections still missing at the rate so far,
    /// `(N̂ − tally) / (tally / elapsed)`, need more than [`FORECAST_MARGIN`]
    /// times the wall left, the enumeration stops with
    /// [`AllsatStop::Forecast`]. A forecast with no estimate (every path cut)
    /// keeps enumerating. `None` never forecasts.
    pub(crate) forecast: Option<Duration>,
}

impl AllsatCtl<'_> {
    /// The plain enumerator, asking for the count forecast at [`FORECAST_AT`]
    /// when `forecast` is set.
    pub(crate) fn with_forecast(forecast: bool) -> Self {
        AllsatCtl { forecast: forecast.then_some(FORECAST_AT), ..Default::default() }
    }

    /// Why the enumeration under this control gave no count, in the words the
    /// stage lines print. A [`AllsatStop::TallyCeiling`] caller states its own
    /// ceiling alongside.
    pub(crate) fn stop_reason(&self) -> &'static str {
        match self.bust {
            Some(AllsatStop::Wall) => "wall budget spent between solves",
            Some(AllsatStop::Terminated) => "wall budget cut a solve in flight",
            Some(AllsatStop::ModelCap) => "model cap",
            Some(AllsatStop::TallyCeiling) => "gate ceiling exceeded",
            Some(AllsatStop::Forecast(_)) => {
                "the estimated count is out of the wall's reach at the current rate"
            }
            None => "declined before enumerating",
        }
    }

    /// Print `{tag}: no count: {reason} (…)` for an enumeration stage that
    /// gave no count after `elapsed_ms`. The tally is printed only when the
    /// stop is not a wall: under a wall it measures the machine's speed. A
    /// forecast stop prints the estimate it was made on, but not its tally:
    /// taken at a fixed time into the run, that too measures the machine.
    pub(crate) fn report_no_count(&self, tag: &str, elapsed_ms: u128) {
        match self.bust {
            Some(AllsatStop::Wall | AllsatStop::Terminated) | None => {
                eprintln!("{tag}: no count: {} ({elapsed_ms}ms)", self.stop_reason());
            }
            Some(AllsatStop::ModelCap | AllsatStop::TallyCeiling) => eprintln!(
                "{tag}: no count: {} ({} projections tallied, {elapsed_ms}ms)",
                self.stop_reason(),
                self.models,
            ),
            Some(AllsatStop::Forecast(estimate)) => eprintln!(
                "{tag}: no count: {} (estimate {estimate:.3e} projections, {elapsed_ms}ms)",
                self.stop_reason(),
            ),
        }
    }
}

/// How far into an enumeration the PMC and PWMC enumerate-show stages take
/// their count forecast ([`AllsatCtl::forecast`]). Early enough that a stop
/// returns most of the wall to the compile behind the stage; late enough that
/// the tally gives a rate.
const FORECAST_AT: Duration = Duration::from_secs(5);

/// Most wall a forecast's path estimate may spend, from the enumeration's
/// own wall.
const FORECAST_WALL: Duration = Duration::from_secs(2);

/// Most random paths a forecast walks, and the cut paths after which it gives
/// up estimating.
const FORECAST_PATHS: u32 = 16;
const FORECAST_MAX_CUT: u32 = 2;

/// How many times the wall left the projections still missing may need, at
/// the rate so far, before a forecast stops the enumeration. The rate only
/// falls as the blocking clauses accumulate, so the forecast already
/// under-reads the time an enumeration needs; the margin covers an estimate
/// that over-reads the count.
const FORECAST_MARGIN: f64 = 2.0;

/// The forecast rule: whether `tally` projections found in `enumerated`, with
/// `left` of the wall to go, are heading for an `estimate` the wall cannot
/// hold. No rate or no wall left keeps enumerating (a spent wall is the wall
/// check's to report).
fn forecast_exceeds_wall(estimate: f64, tally: u64, enumerated: Duration, left: Duration) -> bool {
    let secs = enumerated.as_secs_f64();
    if tally == 0 || secs <= 0.0 || left.is_zero() {
        return false;
    }
    let rate = tally as f64 / secs;
    (estimate - tally as f64) / rate > FORECAST_MARGIN * left.as_secs_f64()
}

/// Take the count forecast of an enumeration of the `constrained` show
/// variables, now, into a wall of `budget` that started at `start`, with
/// `tally` projections found: the estimate when [`forecast_exceeds_wall`]
/// says stop, `None` to keep going, as when every path was cut and there is
/// no estimate.
fn forecast(formula: &CnfFormula, constrained: &[i32], tally: u64, start: Instant, budget: Duration) -> Option<f64> {
    let enumerated = start.elapsed();
    let wall = FORECAST_WALL.min(budget.saturating_sub(enumerated));
    let show: Vec<VarId> = constrained.iter().map(|&d| VarId(d as u32)).collect();
    let estimate = estimate_by_paths(formula, &show, FORECAST_PATHS, FORECAST_MAX_CUT, wall, Some(wall)).mean()?;
    let left = budget.saturating_sub(start.elapsed());
    forecast_exceeds_wall(estimate, tally, enumerated, left).then_some(estimate)
}

/// Projected AllSAT: enumerate the feasible constrained-show projections via
/// blocking clauses, rather than sweeping all 2^|constrained| assignments,
/// adding each to `tally`. Does ≤ (#feasible + 1) solves, so it subsumes
/// [`sweep`] (never more solves) and extends reach to large constrained-show
/// sets whose feasible count is small (e.g. show=24 with only ~7k feasible
/// projections — 2^24 enumeration is hopeless but 7k blocked solves are not).
/// Each blocked SAT result is one distinct feasible projection with a definite
/// polarity per constrained var (a don't-care var is fixed to false this round
/// and its true extension, if feasible, is found in a later round).
///
/// Returns `None` (fall through to compile) when the feasible count exceeds
/// `model_budget`, when the time budget is hit, on an unknown result, or when
/// a requested [`AllsatCtl::forecast`] finds the count out of the wall's
/// reach, never a wrong answer. Soundness is the same as the sweep: the tally, scaled by
/// the free show vars, is exact.
///
/// `ctl` is the caller's control and observation — see [`AllsatCtl`].
fn allsat<T: Tally>(
    formula: &CnfFormula,
    split: &ShowSplit,
    model_budget: u64,
    time_budget: Duration,
    ctl: &mut AllsatCtl<'_>,
    mut tally: T,
) -> Option<T::Value> {
    let ShowSplit { constrained, free } = split;

    let start = Instant::now();
    // Clones share the movable deadline; each solve hands one to a scoped
    // `Bounded` guard, which disconnects before returning. The deadline is
    // reachable through `window.deadline` for the resume path below.
    let term = vitri::sat::WallClockTerminator::new(time_budget);
    let mut solver = load_solver(formula, constrained)?;
    // Budget enforcement (under a wall callback). Without a terminator the
    // loop can only notice the budget between solves, so one long CaDiCaL call
    // runs uninterrupted however far past the budget it likes, spending wall
    // the caller had reserved for something else. The terminator makes the
    // budget bound a single solve too: CaDiCaL returns unknown, which the loop
    // below already treats as a stop.
    let supervised = ctl.at_wall.is_some();
    // The enumeration is quadratic in its own tally: per-model wall grows
    // with the clause database, which gains one blocking clause per model,
    // while per-model search does not. A compact block encoding is slower
    // in both regimes this file serves, so the flat encoding stays.
    let mut count: u64 = 0;
    // Wall currently granted. `at_wall` can push it out (and with it the
    // terminator's deadline) to resume this enumeration rather than restart
    // it — the blocking-clause DB, the learned clauses and `count` all carry
    // over, so a probe-then-commit caller pays for its probe only once. Its
    // ceiling is set only by a `WallGrant` — so the first window (the caller's
    // own opening budget, e.g. the support-enum stage's fixed-cost probe) is
    // never guarded.
    let mut window = Window { budget: time_budget, deadline: term.deadline_handle(), abort_at: None };
    // Pending until the one forecast is taken.
    let mut forecast_at = ctl.forecast;
    loop {
        if start.elapsed() > window.budget && !window.at_wall(ctl, count, start.elapsed()) {
            return bust(ctl, AllsatStop::Wall, count);
        }
        // The count forecast, once, between solves (see `AllsatCtl::forecast`).
        // The estimator loads its own solver, so this one is untouched, and
        // its wall is spent from the enumeration's.
        if let Some(at) = forecast_at
            && count > 0
            && start.elapsed() >= at
        {
            forecast_at = None;
            if let Some(estimate) = forecast(formula, constrained, count, start, window.budget) {
                return bust(ctl, AllsatStop::Forecast(estimate), count);
            }
        }
        let status = if supervised {
            Bounded::new(&mut solver, term.clone()).solve()
        } else {
            solver.solve()
        };
        match status {
            Status::Unsatisfiable => break,
            Status::Unknown => {
                // The only limit connected here is the wall terminator, so
                // an unknown result means a solve was cut at the deadline. Same
                // continuation decision as the between-solves check; CaDiCaL
                // is left in a valid incremental state, so resuming is just
                // the next `solve()`.
                if window.at_wall(ctl, count, start.elapsed()) {
                    continue;
                }
                return bust(ctl, AllsatStop::Terminated, count);
            }
            Status::Satisfiable => {
                count += 1;
                // Tally ceiling, checked before the standing cap so the more
                // specific reason wins when both would trip. The window was
                // granted because the caller asserted the feasible-projection
                // count was at most some bound; a tally past that bound has
                // disproved the assertion the window rests on, so grinding on
                // spends the remainder on a premise this run already refuted.
                // `count` is cumulative across the grant boundary (same
                // solver, same tally), which is the right unit: the bound is
                // on the total feasible set.
                if let Some(limit) = window.abort_at
                    && count > limit
                {
                    return bust(ctl, AllsatStop::TallyCeiling, count);
                }
                if count > model_budget {
                    return bust(ctl, AllsatStop::ModelCap, count);
                }
                // Block this projection: a clause that excludes exactly the
                // found assignment over the constrained-show vars (one clause
                // per projection; see the note on the flat encoding above).
                // val(d)>0 ⇒ var true here ⇒ exclude with ¬d; otherwise (incl.
                // don't-care val==0) treat as false and exclude with +d — the
                // opposite polarity, if feasible, is then found and counted
                // separately.
                // Read every model value first: the first add() leaves the
                // satisfied state, after which val() is illegal.
                let assignment = model_lits(&mut solver, constrained);
                for &lit in &assignment {
                    solver.add(-lit);
                }
                solver.add(0);
                tally.add(&assignment);
            }
        }
    }

    ctl.models = count;
    Some(tally.total(free))
}

/// Estimate the feasible-projection count by drawing random paths one at a
/// time with [`random_path`], until `paths` are drawn, `max_cut` of them are
/// cut, or, when `total` is given, that much wall is spent. Each path runs
/// under `path_wall`, and under what is left of `total`. A formula with no
/// constrained show variable has no path to draw, and a solver that cannot be
/// built draws none; either leaves the estimate unmeasured.
///
/// One at a time so the cut-off costs at most one extra path: an instance
/// whose paths outrun their wall gets out after `max_cut` of them instead of
/// paying for all. Every path loads a fresh solver and gets its own
/// [`path_seed`], so the draw is reproducible. The one path-drawing loop: the
/// support-enum stage's estimate and the [`AllsatCtl::forecast`] both use it.
pub(crate) fn estimate_by_paths(
    formula: &CnfFormula,
    show_vars: &[VarId],
    paths: u32,
    max_cut: u32,
    path_wall: Duration,
    total: Option<Duration>,
) -> PathEstimate {
    let started = Instant::now();
    let mut acc = PathEstimate::default();
    let constrained = split_show(formula, show_vars).constrained;
    if constrained.is_empty() {
        return acc;
    }
    let drawn = |a: &PathEstimate| a.weights.len() as u32 + a.truncated;
    while drawn(&acc) < paths && acc.truncated < max_cut {
        let wall = match total {
            Some(total) => {
                let left = total.saturating_sub(started.elapsed());
                if left.is_zero() {
                    break;
                }
                path_wall.min(left)
            }
            None => path_wall,
        };
        let Some(mut solver) = load_solver(formula, &constrained) else {
            break;
        };
        match random_path(&mut solver, &constrained, wall, path_seed(drawn(&acc))) {
            Path::Weight(weight) => acc.weights.push(weight),
            Path::Cut => acc.truncated += 1,
            Path::Unsatisfiable => {
                acc.unsat = true;
                break;
            }
        }
    }
    acc
}

/// The seed of path `k`. One fixed base, so a re-run of the same instance
/// routes the same way. The engine starts its generator at `seed | 1` and each
/// call loads a fresh solver, so the seeds step by 2 from an odd base: every
/// path gets its own generator state, where a step of 1 would pair them up.
fn path_seed(k: u32) -> u64 {
    const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
    SEED.wrapping_add(2 * u64::from(k))
}

/// The paths [`estimate_by_paths`] drew.
#[derive(Debug, Clone, Default)]
pub(crate) struct PathEstimate {
    /// One unbiased estimate of the feasible-projection count per completed
    /// path (each is a power of two: the number of free branch points taken).
    pub(crate) weights: Vec<f64>,
    /// Paths abandoned because their solves ran past the per-path wall.
    pub(crate) truncated: u32,
    /// The formula itself is unsatisfiable — the projected count is 0.
    pub(crate) unsat: bool,
}

impl PathEstimate {
    /// Mean of the completed paths — the unbiased point estimate of N.
    /// `None` when no path completed (nothing was measured; a caller must not
    /// read that as "small").
    pub(crate) fn mean(&self) -> Option<f64> {
        if self.weights.is_empty() {
            return None;
        }
        Some(self.weights.iter().sum::<f64>() / self.weights.len() as f64)
    }
}

/// One step of the deterministic LCG (Knuth's MMIX constants) the estimators
/// draw from: advances `state` and returns its bits from 33 up.
fn lcg_next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *state >> 33
}

/// How one random path down the projection tree ended.
enum Path {
    /// The path reached a leaf: an unbiased estimate of the feasible-projection
    /// count, a power of two.
    Weight(f64),
    /// A solve ran past the path's wall.
    Cut,
    /// The formula has no model.
    Unsatisfiable,
}

/// Walk one random path down the projection tree of the `constrained` show
/// variables, loaded into `solver`, and estimate the number N of feasible
/// projections from it: the answer to "will this enumeration finish?", asked
/// before spending the wall on it.
///
/// Walk the constrained-show vars in order, maintaining a partial projection
/// that is always satisfiable. At each var, one assumption-solve asks whether
/// the polarity the current model does not take is also feasible:
///   - both feasible → multiply the running weight by 2 and take a uniformly
///     random one of the two branches;
///   - only one feasible → take it, weight unchanged.
///
/// The final weight is an unbiased estimator of N (a path reaches each of the
/// N leaves with probability 1/weight), and the mean over several paths is
/// too. Cost is one solve per show var — the model from the previous solve
/// supplies the other branch for free.
///
/// Why not random restrictions: fixing j show vars to random polarities and
/// scaling by 2^j is also unbiased, but its variance is fatal in exactly this
/// population — when most show vars are near-forced, nearly every restriction
/// is UNSAT, so the estimate collapses to 0 and a caller reading it as "small"
/// commits to an enumeration that will not finish. A path estimate cannot do
/// that: it never returns 0 on a satisfiable formula, and a forced var simply
/// contributes a factor of 1.
///
/// `wall` bounds the path as a whole: its first solve and every branch solve
/// share one deadline, enforced by the same wall terminator the enumerator
/// uses. A path cut mid-solve is [`Path::Cut`] rather than a low weight,
/// because a partial path witnesses nothing: its partial product covers only
/// the solved prefix, so it would pull the mean further below the true count,
/// which this estimator already under-reads (see `TALLY_CEILING` in
/// `stage::mc::support_enum`). `seed` starts the path's coin flips, so the
/// path is reproducible.
fn random_path(solver: &mut CaDiCal, constrained: &[i32], wall: Duration, seed: u64) -> Path {
    let mut state: u64 = seed | 1;
    let mut coin = move || lcg_next(&mut state) & 1 == 1;
    // Cloned into each solve's scoped `Bounded` guard.
    let term = vitri::sat::WallClockTerminator::new(wall);

    // A model of the empty prefix: its show values are the first path.
    match Bounded::new(solver, term.clone()).solve() {
        Status::Satisfiable => {}
        Status::Unsatisfiable => return Path::Unsatisfiable,
        Status::Unknown => return Path::Cut,
    }
    let mut model = model_lits(solver, constrained);
    let mut weight = 1.0f64;
    let mut fixed: Vec<i32> = Vec::with_capacity(constrained.len());
    for i in 0..constrained.len() {
        let taken = model[i];
        for &l in &fixed {
            solver.assume(l);
        }
        solver.assume(-taken);
        // Bound first, so the guard's borrow of the solver ends before an
        // arm reads the model.
        let status = Bounded::new(solver, term.clone()).solve();
        match status {
            Status::Satisfiable => {
                weight *= 2.0;
                if coin() {
                    // Follow the freshly proven branch; this solve's model
                    // supplies feasible values for the vars still ahead.
                    fixed.push(-taken);
                    model[i + 1..].copy_from_slice(&model_lits(solver, &constrained[i + 1..]));
                } else {
                    // Stay on `taken`; the model in hand already agrees
                    // with the whole remaining suffix.
                    fixed.push(taken);
                }
            }
            // Forced: exactly one polarity survives, so no factor.
            Status::Unsatisfiable => fixed.push(taken),
            Status::Unknown => return Path::Cut,
        }
    }
    Path::Weight(weight)
}

/// The wall window an [`allsat`] enumeration runs in: its budget from the
/// start, the terminator deadline behind it, and the tally ceiling of the grant
/// that opened it.
struct Window {
    budget: Duration,
    deadline: vitri::sat::DeadlineHandle,
    abort_at: Option<u64>,
}

impl Window {
    /// Ask the caller whether to keep going past the wall budget. `true` ⇒ the
    /// budget (and the terminator deadline behind it) has been pushed out and
    /// the loop resumes; `false` ⇒ bust. Absent callback ⇒ always bust.
    fn at_wall(&mut self, ctl: &mut AllsatCtl<'_>, count: u64, elapsed: Duration) -> bool {
        let Some(cb) = ctl.at_wall.as_mut() else {
            return false;
        };
        let Some(grant) = cb(count, elapsed) else {
            return false;
        };
        self.budget = elapsed + grant.extra;
        self.deadline.set(Instant::now() + grant.extra);
        // The bound travels with the grant: the terms in force are always the
        // terms of the window actually running.
        self.abort_at = Some(grant.abort_at_models);
        true
    }
}

/// Record why an [`allsat`] enumeration stopped and how far it got, and
/// return its `None`.
fn bust<V>(ctl: &mut AllsatCtl<'_>, reason: AllsatStop, models: u64) -> Option<V> {
    ctl.bust = Some(reason);
    ctl.models = models;
    None
}

/// The literal each of `vars` takes in the solver's current model: `d` when it
/// is true, `-d` otherwise, a don't-care included.
fn model_lits(solver: &mut CaDiCal, vars: &[i32]) -> Vec<i32> {
    vars.iter().map(|&d| if solver.val(d) > 0 { d } else { -d }).collect()
}

/// Random full show-assignments [`feasible_fraction_estimate`] solves.
const FEASIBILITY_SAMPLES: u32 = 64;

/// CaDiCaL conflicts each [`feasible_fraction_estimate`] sample may spend.
const FEASIBILITY_CONFLICT_LIMIT: i32 = 5_000;

/// Quick feasibility-fraction estimate over the constrained-show vars: solve
/// [`FEASIBILITY_SAMPLES`] random full show-assignments (assume every
/// constrained-show var to a pseudo-random polarity) and return the fraction
/// that are SAT: ~1.0 for a near-full projected function, ~0.0 for a sparse
/// one. It is the signal for PMC's near-full skip, whose reasoning is written
/// at the skip ([`show_enum_fastpath`]). Deterministic PRNG (fixed
/// seed) so the routing is reproducible across runs.
///
/// Each sample solve is bounded by [`FEASIBILITY_CONFLICT_LIMIT`] conflicts, so
/// a hard show-assignment (one whose feasibility CaDiCaL cannot settle quickly)
/// returns unknown and is skipped rather than stalling the probe — this makes the
/// estimate fast and stable (a wall-time budget instead makes the result depend on
/// scheduling: a single unbounded UNSAT solve can eat the whole budget and flip
/// the routing run to run). The fraction is over the samples that did settle.
///
/// Returns the fraction of the constrained show assignments that are
/// feasible, or `None` when there is no constrained-show var or when no
/// sample settled within its conflict limit (then the caller does not skip —
/// the enum runs).
fn feasible_fraction_estimate(formula: &CnfFormula, split: &ShowSplit) -> Option<f64> {
    let constrained = &split.constrained;
    if constrained.is_empty() {
        return None;
    }
    let mut solver = load_solver(formula, constrained)?;
    // Deterministic, so the routing is reproducible.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut sat: u32 = 0;
    let mut done: u32 = 0;
    for _ in 0..FEASIBILITY_SAMPLES {
        // Per-solve conflict cap (re-set each solve — CaDiCaL clears it after a
        // completed solve). Unknown ⇒ limit hit ⇒ skip this sample.
        solver.limit(c"conflicts", FEASIBILITY_CONFLICT_LIMIT);
        for &d in constrained {
            let pos = lcg_next(&mut state) & 1 == 1;
            solver.assume(if pos { d } else { -d });
        }
        match solver.solve() {
            Status::Satisfiable => {
                sat += 1;
                done += 1;
            }
            Status::Unsatisfiable => {
                done += 1;
            }
            Status::Unknown => {}
        }
    }
    if done == 0 {
        None
    } else {
        Some(sat as f64 / done as f64)
    }
}

/// Largest constrained-show set the projected AllSAT enumeration takes on.
/// A crude proxy: the real gates are PMC's near-full feasibility skip and the
/// model cap. It is 1024 rather than tighter so an instance with many
/// constrained show variables but few feasible projections, which enumerates
/// in seconds while its compile times out, still reaches the enumeration.
pub(crate) const MAX_ENUM_SHOW: usize = 1024;

/// The bounds on [`show_enum_fastpath`]'s AllSAT run.
#[derive(Clone, Copy)]
pub(crate) struct EnumLimits {
    /// The AllSAT wall in milliseconds; [`allsat_share_ms`] sizes it from the
    /// run's time limit.
    pub(crate) allsat_wall_ms: u64,
    /// The model cap, in place of [`MODEL_CAP`].
    pub(crate) model_budget_override: Option<u64>,
    /// Skip the enumeration when [`feasible_fraction_estimate`] finds the
    /// projection near full, an estimate taken just before it would
    /// enumerate. Only PMC sets it.
    pub(crate) skip_near_full: bool,
    /// The run's wall. Neither the sweep nor AllSAT runs past it, whatever
    /// its own budget, and once it has passed neither starts.
    pub(crate) deadline: Option<Instant>,
}

/// An AllSAT wall of `share` of the run's time limit, clamped to 10 s..1800 s;
/// 60 s when the run has none.
pub(crate) fn allsat_share_ms(time_limit_ms: Option<u64>, share: f64) -> u64 {
    crate::budget::resolve_scaled(time_limit_ms, share, 60_000, 10_000, 1_800_000)
}

/// Counts the show set by enumeration: a 2^K sweep first, then, when the sweep
/// declines and the constrained show set has at most [`MAX_ENUM_SHOW`]
/// variables, a projected AllSAT enumeration of the feasible projections.
/// PMC, PWMC and the Arjun-reduced support enumeration each pass their own
/// `tally`, [`Models`] or [`WeightedModels`], and their own `ctl`,
/// which controls the AllSAT run and afterwards reports how it ended. AllSAT
/// runs at most once, so `ctl` describes that run, and is untouched when it did
/// not run. `limits` bounds that run.
///
/// Returns `(value, method, elapsed_ms)`, or `None` to fall through to compile;
/// hitting either cap gives `None`, never a wrong count.
pub(crate) fn show_enum_fastpath<T: Tally + Clone>(
    formula: &CnfFormula,
    show: &ShowSet,
    tally: T,
    limits: EnumLimits,
    ctl: &mut AllsatCtl<'_>,
) -> Option<(T::Value, &'static str, u128)> {
    let EnumLimits { allsat_wall_ms, model_budget_override, skip_near_full, deadline } = limits;
    if crate::budget::deadline_passed(deadline) {
        return None;
    }
    let t_enum = Instant::now();
    let split = split_show(formula, &show_vars(show));
    let mut result = sweep(formula, &split, tally.clone(), deadline).map(|v| (v, "small-show enumeration"));

    // The sweep also declines at small constrained-show counts when a large
    // formula does not fit in `SWEEP_WALL`, so the AllSAT band starts at 1.
    // AllSAT enumerates only the feasible projections; the model cap sends an
    // instance with many of them back to compile.
    if result.is_none() {
        let constrained_show = split.constrained.len();
        if (1..=MAX_ENUM_SHOW).contains(&constrained_show) {
            let model_budget = model_budget_override.unwrap_or(MODEL_CAP);
            // The wall, its 10 s floor included, stops at the run's. Without
            // a run wall it stands as given, zero included: a supervising
            // caller's callback decides at the wall whether to go on.
            let wall_ms = crate::budget::left_ms(deadline)
                .map_or(Some(allsat_wall_ms), |left_ms| (left_ms > 0).then_some(allsat_wall_ms.min(left_ms)));
            // Near-full skip. The AllSAT enum only wins when the feasible
            // projection set is small. A near-full projected function (feasible
            // ≈ 2^|show|, e.g. count 2^32−10) makes the enum grind to its model
            // cap and then bail, starving the compile path that actually solves
            // these, so a quick random-sample feasibility estimate routes these
            // to compile with the full wall. The signal is monotone-safe: a high
            // fraction ⇒ large feasible set ⇒ the enum could not finish under
            // the cap anyway. Skip when the estimated feasible count,
            // frac · 2^constrained, exceeds 8 × the model cap (compared in log2
            // to avoid overflow).
            let near_full = || {
                skip_near_full
                    && feasible_fraction_estimate(formula, &split).is_some_and(|frac| {
                        frac > 0.0 && frac.log2() + constrained_show as f64 > (8.0 * model_budget as f64).log2()
                    })
            };
            if let Some(wall_ms) = wall_ms
                && !near_full()
            {
                let allsat_budget = std::time::Duration::from_millis(wall_ms);
                result = allsat(formula, &split, model_budget, allsat_budget, ctl, tally)
                    .map(|v| (v, "projected ALL-SAT"));
            }
        }
    }
    result.map(|(v, method)| (v, method, t_enum.elapsed().as_millis()))
}

#[cfg(test)]
#[path = "tests/enumeration.rs"]
mod tests;
