//! What the two projected counts, PMC and PWMC, share: the seam they are
//! written on, a list of stages run in order until one answers; the vitri
//! projection step, the trivial residual and the show-set enumeration, stages
//! of both; the line their compiles print; and the show-aware counting vtree
//! both compile on.
//!
//! A stage sees the problem as the stages before it left it, and the trace of
//! what they spent; the runner hands it the run and whatever else it reads. It
//! answers in the problem's own space; the seam folds the problem's lift once,
//! at the answer, and measures there what the record keeps beside it. A stage
//! that rewrites the problem hands back the reduction alone, and the seam
//! composes its lift with the one already carried.

use std::sync::Arc;
use std::time::Instant;

use tididi::vtree::Vtree;
use vitri::cnf::CnfFormula;

use super::Run;
use crate::vtree::VtreeOptions;
use crate::enumeration::{AllsatCtl, EnumLimits, Tally, show_enum_fastpath};
use crate::output::Projected;
use crate::problem::{Domain, Problem, ShowSet};

/// What one stage decided.
pub(super) enum Step<D: Domain> {
    /// The answer, in the current problem's space, before its lift.
    Answer(D::Value),
    /// The problem, reduced; its lift is this rewrite's alone.
    Rewrite(Problem<D>),
    /// Nothing; the next stage runs.
    Decline,
}

/// What the stages so far spent and started from, for the lines a later stage
/// prints.
pub(super) struct Trace {
    /// The instance's variable and clause counts as it arrived.
    pub(super) input_vars: u32,
    pub(super) input_clauses: usize,
    /// Wall spent in the rewrites so far.
    pub(super) preprocess_ms: u128,
}

impl Trace {
    /// The line a projected compile of `work` prints when it answers, under
    /// the mode's `tag`: the preprocessing behind it, its vtree and compile
    /// walls, how many projected variables it `forgot` in the diagram and how
    /// many variables `show` holds.
    pub(super) fn print_compile(
        &self,
        tag: &str,
        work: &CnfFormula,
        vtree_ms: u128,
        compile_ms: u128,
        forgot: usize,
        show: usize,
    ) {
        eprintln!(
            "{tag} compile: preprocessing {}ms ({}c→{}c), vtree {vtree_ms}ms, compile {compile_ms}ms \
             ({forgot} projected variables forgotten in the diagram), show {show}, vars {}",
            self.preprocess_ms,
            self.input_clauses,
            work.clauses().len(),
            self.input_vars,
        );
    }
}

/// One stage over the domain `D`.
pub(super) type Stage<'s, D> = &'s dyn Fn(&Problem<D>, &Trace) -> Step<D>;

/// Run `stages` in order on `problem`; the first answer, lifted, with the
/// show set it was answered over and the stages' wall less the rewrites' and
/// the vtree construction's, which the run's `vtree_time` holds. Checks the
/// run's deadline on both sides of each stage, including shortcuts and native
/// preprocessing calls.
///
/// # Errors
///
/// [`crate::Error::Incomplete`] when every stage declined, each having said
/// why, or the deadline expired: there is no other path behind the stages.
pub(super) fn run_stages<D: Domain>(
    problem: Problem<D>,
    stages: &[Stage<'_, D>],
    deadline: Option<Instant>,
    vtree_time: &crate::budget::VtreeTime,
) -> Result<Projected<D::Value>, crate::Error> {
    let mut trace = Trace {
        input_vars: problem.formula.num_vars(),
        input_clauses: problem.formula.clauses().len(),
        preprocess_ms: 0,
    };
    let clock = crate::budget::CompileClock::start(vtree_time);
    let mut problem = problem;
    for stage in stages {
        crate::budget::check_deadline(deadline)?;
        let started = Instant::now();
        let step = stage(&problem, &trace);
        crate::budget::check_deadline(deadline)?;
        match step {
            Step::Answer(value) => {
                return Ok(Projected {
                    value: D::lifted(value, &problem.lift),
                    show_vars: problem.show.as_ref().map(ShowSet::len),
                    compile_ms: clock.compile_ms(trace.preprocess_ms as u64),
                });
            }
            Step::Rewrite(next) => {
                trace.preprocess_ms += started.elapsed().as_millis();
                problem = problem.rewritten(next);
            }
            Step::Decline => {}
        }
    }
    Err(crate::Error::every_stage_gave_up())
}

/// Count `show`'s feasible assignments by enumeration
/// ([`show_enum_fastpath`]) under `limits`, adding them up with `tally`, and
/// print under the mode's `tag` what answered, or why nothing did. `None` when
/// the enumeration gave no count. `forecast` is the AllSAT run's
/// [`AllsatCtl::forecast`]: the enumerate-show stages ask for one, so that an
/// enumeration heading for more projections than its wall holds returns the
/// rest of that wall to the compile; the component paths do not.
pub(super) fn enumerate_show<T: Tally + Clone>(
    tag: &str,
    formula: &CnfFormula,
    show: &ShowSet,
    tally: T,
    limits: EnumLimits,
    forecast: Option<std::time::Duration>,
) -> Option<T::Value> {
    let mut ctl = AllsatCtl { forecast, ..Default::default() };
    let started = Instant::now();
    let Some((value, method, elapsed_ms)) = show_enum_fastpath(formula, show, tally, limits, &mut ctl) else {
        ctl.report_no_count(&format!("{tag} enumerate-show"), started.elapsed().as_millis());
        return None;
    };
    eprintln!("{tag} enumerate-show: {method} ({} show vars, {elapsed_ms}ms)", show.len());
    Some(value)
}

/// The projected Arjun budget, of PMC's minimization and PWMC's preprocessing
/// alike: 38% of the run's declared wall, 50% on a formula of a million
/// variables or more, clamped to `[5s, 300s]`; 300 s without a wall. The stage
/// cuts it to what the run has left ([`crate::budget::clamped_to_deadline_ms`]).
///
/// A share of the wall, so that under a short limit a reduction that does not
/// converge cannot spend all of it: Arjun's oracle pass runs to its deadline.
/// A large share, because Arjun stops the moment it converges: more budget
/// costs only the reductions that never converge, while a reduction cut off
/// just short of converging leaves the raw formula, which is far slower to
/// count.
pub(super) fn projected_arjun_budget_ms(time_limit_ms: Option<u64>, num_vars: u64) -> u64 {
    let frac: u64 = if num_vars >= 1_000_000 { 50 } else { 38 };
    time_limit_ms
        .map(|w| (w * frac / 100).clamp(5_000, 300_000))
        .unwrap_or(300_000)
}

/// One vitri preprocessing stage on `problem`, as a pipeline step: UNSAT is
/// the answer zero, a reduction is a rewrite that prints what it kept and its
/// wall, and an error prints `{tag}: {error} — keeping {kept}` and declines.
/// `tag` is `[<mode>] <stage>`.
pub(super) fn projection_step<D: Domain>(
    problem: &Problem<D>,
    weights: Option<&vitri::cnf::WeightTable>,
    config: &vitri::RunConfig,
    tag: &str,
    kept: &str,
) -> Step<D> {
    let started = Instant::now();
    match crate::frontend::record::preprocess_problem::<D>(
        &problem.formula,
        problem.show.as_ref(),
        weights,
        config,
    ) {
        Ok((crate::frontend::record::Outcome::Unsat, _)) => Step::Answer(D::zero()),
        Ok((crate::frontend::record::Outcome::Reduced(reduced), _)) => {
            eprintln!(
                "{tag}: show {}→{}, vars {}→{}, clauses {}→{} ({}ms)",
                problem.show_len(),
                reduced.show_len(),
                problem.formula.num_vars(),
                reduced.formula.num_vars(),
                problem.formula.clauses().len(),
                reduced.formula.clauses().len(),
                started.elapsed().as_millis(),
            );
            Step::Rewrite(reduced)
        }
        Err(error) => {
            eprintln!("{tag}: {error} — keeping {kept}");
            Step::Decline
        }
    }
}

/// The residual nothing can compile: an empty clause, or a formula over no
/// variables. Answers `0` for the first and `1` for the second — every show
/// variable of a 0-variable formula is gone with it — and prints which it
/// was under `tag`, `[<mode>] trivial`.
pub(super) fn trivial_residual<D: Domain>(
    tag: &str,
    problem: &Problem<D>,
    trace: &Trace,
) -> Step<D> {
    let work = &problem.formula;
    let has_empty_clause = work.clauses().iter().any(|c| c.literals.is_empty());
    if !has_empty_clause && work.num_vars() != 0 {
        return Step::Decline;
    }
    let what = if has_empty_clause { "an empty clause" } else { "no variables left" };
    eprintln!(
        "{tag}: {what} (preprocessing {}ms, {}c→{}c), show {}, vars {}",
        trace.preprocess_ms,
        trace.input_clauses,
        work.clauses().len(),
        problem.show_len(),
        trace.input_vars,
    );
    Step::Answer(if has_empty_clause { D::zero() } else { D::one() })
}

/// Build the counting vtree, with the variables it places: peak
/// ∃-forget-frontier selection when there is a show set, ordinary clause-load
/// balance otherwise, on the run's spec and policy; `None` when the
/// construction failed. PWMC's compile uses it too; the conditioning branches
/// build theirs with `count_vtree` on vitri's portfolio and no run policy.
pub(super) fn counting_vtree(
    run: &Run<'_>,
    work: &CnfFormula,
    show: Option<&ShowSet>,
) -> Option<CountVtree> {
    let ctx = if show.is_some() { VtreeOptions::by_peak_width() } else { VtreeOptions::default() };
    count_vtree(
        run,
        work,
        run.config.vtree_spec(),
        &ctx.with_deadline(crate::budget::vtree_deadline(run.deadline))
            .with_run_policy(run.config),
        show,
    )
}

/// Build `spec`'s vtree for `work` under `ctx`, with the variables it places,
/// or `None` when the construction failed. The projected ones are the leaves
/// outside `show`; none without one.
pub(super) fn count_vtree(
    run: &Run<'_>,
    work: &CnfFormula,
    spec: &str,
    ctx: &VtreeOptions,
    show: Option<&ShowSet>,
) -> Option<CountVtree> {
    let built = run.config.vtree_time.time(|| crate::vtree::build_vtree_with_components(work, spec, ctx));
    let crate::vtree::VtreeBuild { root: vtree, components: component_vtrees } =
        crate::vtree::built_or_give_up(built)?.bind(run.eng);
    let projected = show.map_or_else(Vec::new, |show| {
        vtree
            .leaf_bottomup()
            .map(|(_, v)| v)
            .filter(|&v| !show.contains(crate::convert::formula_var(v)))
            .collect()
    });
    Some(CountVtree { vtree, component_vtrees, projected })
}

/// The counting vtree and the variables it places.
pub(super) struct CountVtree {
    pub(super) vtree: Arc<Vtree>,
    pub(super) component_vtrees: Option<Vec<crate::vtree::ComponentVtree>>,
    /// The vtree's leaves outside the show set, bottom-up; empty without a
    /// show set.
    pub(super) projected: Vec<tididi::vtree::VarId>,
}

#[cfg(test)]
#[path = "tests/projected.rs"]
mod tests;
