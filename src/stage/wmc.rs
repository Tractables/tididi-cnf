//! The weighted model count.
//!
//! The weighted count starts from the parsed formula. Vitri owns the
//! simplified formula's weighted simplify/DVE record, exact lift, reduced
//! weights and portfolio vtree; TiDiDi owns only the order — Arjun's
//! reduction first, then the simplified formula — and the compiles.

use std::time::Instant;

use vitri::cnf::CnfFormula;
use crate::compile::WeightedCount;
use crate::problem::Weighted;
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
/// component vtrees for `formula` when it has them, otherwise a fresh
/// `portfolio` vtree whose construction is bounded by `deadline`; then the
/// rational semiring over `weights` (reduced-space `(w_neg, w_pos)` pairs) and
/// the marginalizing count, with the diagram's verdict. A panic, such as an
/// allocation failure, is logged as `[wmc] compile: panicked` and returns `None`,
/// handing the time to the next attempt; so does a vtree that cannot be built.
fn compile_weighted(
    run: &Run<'_>,
    formula: &CnfFormula,
    weights: &[(num_rational::BigRational, num_rational::BigRational)],
    prebuilt: Option<VtreeBuild>,
    deadline: Option<Instant>,
) -> Option<WeightedCount<WeightValue>> {
    let eng = run.eng;
    run_isolated("[wmc] compile", || {
        let VtreeBuild { root: vtree, components } = match prebuilt {
            Some(built) => built,
            None => crate::vtree::built_or_give_up(crate::vtree::build_vtree_with_components(
                formula,
                super::PORTFOLIO_VTREE_SPEC,
                &VtreeOptions::default()
                    .with_deadline(crate::budget::vtree_deadline(deadline))
                    .with_portfolio_history(run.config.portfolio_history.clone()),
            ))?
            .bind(eng),
        };
        crate::compile::weighted_count(
            eng,
            formula,
            &vtree,
            crate::compile::ComponentPlan::from_components(components.as_deref()),
            crate::convert::rational_weight_algebra(weights),
            ARITHMETIC,
            deadline,
        )
    })
}

/// The weighted model count.
///
/// Both preprocessing attempts start from the parsed formula. The simplified
/// formula is one Vitri WMC frontend bundle, so its formula, complete weights,
/// exact lift and portfolio vtree cannot drift apart at the compiler boundary.
/// `declared_weights` is the instance's `c p weight` table; a literal it
/// leaves out weighs one.
pub(crate) fn run_wmc_count(
    run: &Run<'_>,
    formula: &CnfFormula,
    declared_weights: Option<&vitri::cnf::WeightTable>,
) -> Result<WeightedCount<WeightValue>, crate::Error> {
    let program_deadline = run.deadline;
    // The domain decides which exact-only rewrites can run, so the log says it.
    eprintln!("[wmc] arithmetic: log (bounded)");
    // The declared table as vitri reads it: its count chain resolves every
    // literal the table leaves out to one.
    let orig_table = declared_weights.cloned().unwrap_or_default();
    let crate::frontend::WmcFrontend { outcome, vtree, report } =
        crate::frontend::wmc_frontend(formula, &orig_table, run.config, program_deadline)
            .map_err(crate::Error::stage_failed("wmc preprocessing"))?;
    let render_report = |report: &crate::frontend::record::Report, result: &str| {
        eprintln!(
            "[wmc] preprocessing: simplify={:?} arjun={:?}; {}ms; {result}",
            report.stages.simplify,
            report.stages.arjun,
            report.telemetry.total_ms,
        );
    };
    let (simplified_problem, simplified_vtree) = match (outcome, vtree) {
        (Outcome::Unsat, _) => {
            render_report(&report, "UNSAT");
            return Ok(WeightedCount::Unsatisfiable);
        }
        // Nothing is left to compile and nothing was refuted: every clause is
        // satisfied, and the lift is the whole count.
        (Outcome::Reduced(problem), None) => {
            render_report(&report, "fully resolved");
            return Ok(WeightedCount::Satisfiable(weight_value(problem.lift)));
        }
        (Outcome::Reduced(problem), Some(vtree)) => {
            render_report(
                &report,
                &format!(
                    "{}v/{}c reduced",
                    problem.formula.num_vars(),
                    problem.formula.clauses().len()
                ),
            );
            // Bound before any diagram is built on it, as every fresh tree is.
            (problem, vtree.bind(run.eng))
        }
    };
    let t_compile = Instant::now();

    // One CPU, in order: reduce with a short budget, compile the reduction,
    // and on an inert reduction or a give-up compile the simplified formula on
    // the reserved time. Each answers instances the other does not: an inert
    // or lossy reduction declines at once and leaves the simplified formula
    // the whole budget, and a useful one compiles first, bounded.

    // The simplified formula: the Vitri WMC frontend's exact reduced formula,
    // weights, vtree and scalar lift, compiled whole.
    let count_simplified = |deadline: Option<Instant>| -> Option<WeightedCount<WeightValue>> {
        compile_weighted(
            run,
            &simplified_problem.formula,
            &simplified_problem.weights,
            Some(simplified_vtree),
            deadline,
        )
        .map(|raw| raw.map(|raw| raw.mul_rational(&simplified_problem.lift)))
    };

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
    let value = count_reduced(run, formula, &orig_table, simplified_clauses, primary_deadline)
    .or_else(|| {
        eprintln!("[wmc] arjun: no answer; compiling the simplified formula on the reserved time");
        count_simplified(program_deadline)
    });

    // There is no other path behind the two attempts, so the run ends
    // without a count.
    let Some(value) = value else {
        return Err(crate::Error::compile_gave_up());
    };
    eprintln!("[wmc] compile: counted in {}ms", t_compile.elapsed().as_millis());
    Ok(value)
}

/// The reduced attempt: Arjun runs through Vitri on the original formula and
/// original weights. It never touches the DVE-reduced simplified formula, whose
/// clause count is `simplified_clauses`; Vitri owns the weighted-usability and caller-baseline
/// gates, and its exact rational lift alone maps the reduced WMC back to the
/// original formula.
fn count_reduced(
    run: &Run<'_>,
    formula: &CnfFormula,
    orig_table: &vitri::cnf::WeightTable,
    simplified_clauses: usize,
    compile_deadline: Option<Instant>,
) -> Option<WeightedCount<WeightValue>> {
    // The reduce budget, cut to the window this attempt was given
    // (`compile_deadline`: `ARJUN_SHARE` of what remains, the rest reserved
    // for the simplified formula). Nothing left ⇒ the attempt declines, the
    // "no usable reduction" path, and the simplified formula gets the time.
    let reduce_budget = crate::budget::clamped_to_deadline_ms(
        crate::budget::arjun_budget_ms(run.config.time_limit_ms, 300_000),
        compile_deadline,
        Instant::now(),
    )
    .map(std::time::Duration::from_millis)?;
    // Vitri owns the weighted-usability and caller-baseline gates; only
    // a reduction its Arjun stage kept is compiled here.
    let arjun_config = crate::frontend::request::arjun_stage(
        run.config,
        vitri::cnf::Mode::Wmc,
        run.deadline,
        reduce_budget,
        vitri::config::ArjunClauseGrowth::RejectAgainst(simplified_clauses),
    );
    let (arj_formula, arj_w, k) = match crate::frontend::record::preprocess_problem::<Weighted>(
        formula,
        None,
        Some(orig_table),
        &arjun_config,
    ) {
        Ok((Outcome::Reduced(problem), report))
            if matches!(report.stages.arjun, Some(vitri::bundle::StageOutcome::Ran)) =>
        {
            (problem.formula, problem.weights, problem.lift)
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
        arj_formula.num_vars(),
        arj_formula.clauses().len(),
        k.numer(),
        k.denom(),
    );
    // No prebuilt vtree: the compile builds one on the reduction. A compile
    // that runs out of memory gives up fast, leaving the simplified formula
    // the reserved time.
    compile_weighted(run, &arj_formula, &arj_w, None, compile_deadline)
        .map(|raw| raw.map(|raw| raw.mul_rational(&k)))
}

/// A weighted answer in [`ARITHMETIC`], the domain the compiles answer in.
fn weight_value(value: num_rational::BigRational) -> WeightValue {
    WeightValue::log(SignedLog::from_rational(&value))
}

#[cfg(test)]
#[path = "tests/wmc.rs"]
mod tests;
