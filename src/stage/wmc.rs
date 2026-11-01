//! The weighted model count.
//!
//! The weighted count starts from the parsed formula. vitri owns the
//! simplified formula's weighted simplify/DVE record, exact lift, reduced
//! weights and portfolio vtree; this crate owns only the order — Arjun's
//! reduction first, then the simplified formula — and the compiles. A vitri
//! bundle's count is one compile of its reduced formula on its vtree
//! ([`run_wmc_bundle`]).

use std::time::Instant;

use vitri::cnf::CnfFormula;
use crate::problem::{Problem, Weighted, WeightedCount};
use crate::frontend::record::Outcome;

use crate::vtree::{VtreeBuild, VtreeOptions};
use tididi::diagram::{Arithmetic, SignedLog, WeightValue};

use super::{Run, run_isolated};

/// The domain every weighted count of this mode runs in: the bounded log
/// domain. The answer carries a tolerance, and log arithmetic avoids the digit
/// growth of exact rationals.
const ARITHMETIC: Arithmetic = Arithmetic::SignedLog;

/// The share of the time left, from when the compiles start, that the reduced
/// attempt may use before the simplified formula takes over. The simplified
/// formula answers most instances, so it keeps the larger part; the reduced
/// attempt's answers are fast compiles after a useful reduction, which need
/// only a short window.
const ARJUN_SHARE: f64 = 0.25;

/// One weighted compile under panic isolation: the caller's bound root and
/// component vtrees for `problem.formula` when it has them, otherwise a fresh
/// `portfolio` vtree whose construction is bounded by `deadline`; then the
/// rational semiring over `problem.weights` (reduced-space `(w_neg, w_pos)`
/// pairs), the marginalizing count, and `problem.lift` applied to the
/// diagram's answer. A panic, such as an allocation failure, is logged as
/// `[wmc] compile: panicked` and returns `None`, handing the time to the next
/// attempt; so does a vtree that cannot be built.
fn compile_weighted(
    run: &Run<'_>,
    problem: &Problem<Weighted>,
    prebuilt: Option<VtreeBuild>,
    deadline: Option<Instant>,
) -> Option<WeightedCount<WeightValue>> {
    let eng = run.eng;
    run_isolated("[wmc] compile", || {
        let VtreeBuild { root: vtree, components } = match prebuilt {
            Some(built) => built,
            None => crate::vtree::built_or_give_up(run.config.vtree_time.time(|| {
                crate::vtree::build_vtree_with_components(
                    &problem.formula,
                    super::PORTFOLIO_VTREE_SPEC,
                    &VtreeOptions::default()
                        .with_deadline(crate::budget::vtree_deadline(deadline))
                        .with_run_policy(run.config),
                )
            }))?
            .bind(eng),
        };
        crate::compile::weighted_count(
            eng,
            &problem.formula,
            &vtree,
            crate::compile::ComponentPlan::from_components(components.as_deref()),
            crate::convert::rational_weight_algebra(&problem.weights),
            ARITHMETIC,
            deadline,
        )
        .map(|raw| raw.map(|raw| raw.mul_rational(&problem.lift)))
    })
}

/// The weighted model count.
///
/// Both preprocessing attempts start from the parsed formula. The simplified
/// formula is one vitri WMC frontend bundle, so its formula, complete weights,
/// exact lift and portfolio vtree cannot drift apart at the compiler boundary.
/// `declared_weights` is the instance's `c p weight` table; a literal it
/// leaves out weighs what [`vitri::cnf::WeightTable::resolve`] gives it.
pub(crate) fn run_wmc_count(
    run: &Run<'_>,
    formula: &CnfFormula,
    declared_weights: Option<&vitri::cnf::WeightTable>,
) -> Result<WeightedCount<WeightValue>, crate::Error> {
    let program_deadline = run.deadline;
    // The domain decides which exact-only rewrites can run, so the log says it.
    eprintln!("[wmc] arithmetic: log (bounded)");
    // The declared table as vitri reads it: its count chain resolves every
    // literal the table leaves out.
    let orig_table = declared_weights.cloned().unwrap_or_default();
    let crate::frontend::WmcFrontend { outcome, vtree, report } =
        crate::frontend::wmc_frontend(formula, &orig_table, run.config, program_deadline)
            .map_err(crate::Error::stage_failed("wmc preprocessing"))?;
    let (simplified_problem, simplified_vtree) = match (outcome, vtree) {
        (Outcome::Unsat, _) => {
            render_wmc_report(&report, "UNSAT");
            return Ok(WeightedCount::Unsatisfiable);
        }
        (Outcome::Reduced(problem), None) => {
            render_wmc_report(&report, "fully resolved");
            return Ok(WeightedCount::Satisfiable(weight_value(problem.lift)));
        }
        (Outcome::Reduced(problem), Some(vtree)) => {
            render_wmc_report(
                &report,
                &format!(
                    "{}v/{}c reduced",
                    problem.formula.num_vars(),
                    problem.formula.clauses().len()
                ),
            );
            (problem, vtree.bind(run.eng))
        }
    };
    let t_compile = Instant::now();

    // One CPU, in order: reduce with a short budget, compile the reduction,
    // and on an inert reduction or a give-up compile the simplified formula on
    // the reserved time. Each answers instances the other does not: an inert
    // or lossy reduction declines at once and leaves the simplified formula
    // the whole budget, and a useful one compiles first, bounded.

    // The reduction's budget is derived inside `count_reduced`, which needs
    // the deadline. Its compile is bounded by `primary_deadline`, which reserves
    // time for the simplified formula when Arjun reduces but does not solve.
    let primary_deadline = program_deadline.map(|deadline| {
        let now = Instant::now();
        now + deadline.saturating_duration_since(now).mul_f64(ARJUN_SHARE)
    });

    // The reduced attempt on its share of the time, then the simplified
    // formula on all of it when the reduced attempt declined or gave up.
    let simplified_clauses = simplified_problem.formula.clauses().len();
    let value = count_reduced(run, formula, &orig_table, simplified_clauses, primary_deadline).or_else(|| {
        eprintln!("[wmc] arjun: no answer; compiling the simplified formula on the reserved time");
        compile_weighted(run, &simplified_problem, Some(simplified_vtree), program_deadline)
    });

    // There is no other path behind the two attempts, so the run ends
    // without a count.
    let Some(value) = value else {
        return Err(crate::Error::compile_gave_up());
    };
    eprintln!("[wmc] compile: counted in {}ms", t_compile.elapsed().as_millis());
    Ok(value)
}

fn render_wmc_report(report: &crate::frontend::record::Report, result: &str) {
    eprintln!(
        "[wmc] preprocessing: simplify={:?} arjun={:?}; {}ms; {result}",
        report.stages.simplify,
        report.stages.arjun,
        report.telemetry.total_ms,
    );
}

/// The reduced attempt: Arjun runs through vitri on the original formula and
/// original weights. It never touches the DVE-reduced simplified formula, whose
/// clause count is `simplified_clauses`; vitri owns the weighted-usability and
/// caller-baseline gates, so only a reduction its Arjun stage kept is compiled
/// here, and its exact rational lift alone maps the reduced WMC back to the
/// original formula.
fn count_reduced(
    run: &Run<'_>,
    formula: &CnfFormula,
    orig_table: &vitri::cnf::WeightTable,
    simplified_clauses: usize,
    compile_deadline: Option<Instant>,
) -> Option<WeightedCount<WeightValue>> {
    // The reduce budget, the front end's own Arjun budget for the run's time
    // limit (`ArjunBudget::derived_duration`), cut to the window this attempt
    // was given (`compile_deadline`: `ARJUN_SHARE` of what remains, the rest
    // reserved for the simplified formula). Nothing left ⇒ the attempt
    // declines, the "no usable reduction" path, and the simplified formula
    // gets the time.
    let reduce_budget = crate::budget::clamped_to_deadline_ms(
        vitri::config::ArjunBudget::derived_duration(run.config.time_limit_ms).as_millis() as u64,
        compile_deadline,
        Instant::now(),
    )
    .map(std::time::Duration::from_millis)?;
    let arjun_config = crate::frontend::request::arjun_stage(
        run.config,
        vitri::cnf::Mode::Wmc,
        run.deadline,
        reduce_budget,
        vitri::config::ArjunClauseGrowth::RejectAgainst(simplified_clauses),
    );
    let reduced = match crate::frontend::record::preprocess_problem::<Weighted>(
        formula,
        None,
        Some(orig_table),
        &arjun_config,
    ) {
        Ok((Outcome::Reduced(problem), report))
            if matches!(report.stages.arjun, Some(vitri::bundle::StageOutcome::Ran)) =>
        {
            problem
        }
        Ok((Outcome::Unsat, report))
            if matches!(report.stages.arjun, Some(vitri::bundle::StageOutcome::Ran)) =>
        {
            eprintln!("[wmc] arjun: refuted the formula");
            return Some(WeightedCount::Unsatisfiable);
        }
        _ => {
            eprintln!("[wmc] arjun: no usable reduction — skipped");
            return None;
        }
    };
    eprintln!(
        "[wmc] arjun: reduced {}v/{}c → {}v/{}c (×{}/{})",
        formula.num_vars(),
        formula.clauses().len(),
        reduced.formula.num_vars(),
        reduced.formula.clauses().len(),
        reduced.lift.numer(),
        reduced.lift.denom(),
    );
    // No prebuilt vtree: the compile builds one on the reduction. A compile
    // that runs out of memory gives up fast, leaving the simplified formula
    // the reserved time.
    compile_weighted(run, &reduced, None, compile_deadline)
}

/// The weighted count of a vitri bundle, `outcome` as its record describes
/// it: refuted, or one compile of its reduced formula on the bundle's vtree
/// (`Config::bundle_tree`), with nothing preprocessed, and the record's exact
/// lift applied to the answer. Nothing follows a compile that gives up.
pub(crate) fn run_wmc_bundle(
    run: &Run<'_>,
    outcome: Outcome<Weighted>,
) -> Result<WeightedCount<WeightValue>, crate::Error> {
    eprintln!("[wmc] arithmetic: log (bounded)");
    let problem = match outcome {
        Outcome::Unsat => {
            eprintln!("[wmc] preprocessing: UNSAT");
            return Ok(WeightedCount::Unsatisfiable);
        }
        Outcome::Reduced(problem) => problem,
    };
    // A bundle with no vtree has no variable left, and the lift is the count.
    let Some(built) = run.config.bundle_tree() else {
        eprintln!("[wmc] preprocessing: fully resolved");
        return Ok(WeightedCount::Satisfiable(weight_value(problem.lift)));
    };
    let vtree = built.clone().bind(run.eng);
    compile_weighted(run, &problem, Some(vtree), run.deadline)
        .ok_or_else(crate::Error::compile_gave_up)
}

/// A weighted answer in [`ARITHMETIC`], the domain the compiles answer in.
fn weight_value(value: num_rational::BigRational) -> WeightValue {
    WeightValue::log(SignedLog::from_rational(&value))
}

#[cfg(test)]
#[path = "tests/wmc.rs"]
mod tests;
