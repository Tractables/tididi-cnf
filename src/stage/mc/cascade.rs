//! The primary compile and the fallbacks that run when it fails.
//!
//! The primary compile runs first. If one of its internal steps stalls, runs
//! past the per-step wall of the stall rule, it is given up and the same
//! reduction is compiled again on a `goatd-primal` tree: the stall recompile.
//! If that answers nothing too, or the primary failed any other way, the
//! fallbacks run — the no-SBVA re-reduce, support enumeration, the unreduced
//! retry — and then Shannon split, as [`count_with_recovery`] orders them.
//! Each function below is one step of that order and says what it decides.
//! Every step that answers folds its own `2^k` multiplier before returning,
//! so the count leaving this module is always in `compile_formula` space.

use std::time::Instant;

use vitri::cnf::CnfFormula;
use crate::compile::{CompileRun, StallRule, StallWatch, release_failed_compile};
use crate::config::Config;
use crate::output::AnsweredBy;
use crate::stage::{PORTFOLIO_VTREE_SPEC, RETRY_VTREE_SPEC, Run};

use super::windows::{
    UNREDUCED_RETRY_RESERVE, no_sbva_retry_want, no_sbva_retry_window, unreduced_retry_window,
};
use super::attempt::{
    CompileAttempt, attempt_from_vitri_build, count_attempt, portfolio_attempt,
};
use super::recovery::{Counted, Recovered, count_with_recovery};
use super::support_enum::support_enum_stage;

/// A Shannon cofactor's compile, in both cascades: a [`portfolio_attempt`], every
/// component's vtree built up front by vitri, announced on stderr. The run's
/// wall bounds the vtree builds and the compile alike. A vtree that cannot be
/// built gives up with `None` before the compile, as a compile that gives up
/// does.
pub(super) fn count_cofactor(run: &Run<'_>, form: &CnfFormula) -> Option<Counted> {
    // Effective support: the vars that occur in some clause.
    let eff_support = crate::formula::occurrence_counts(form, 0).iter().filter(|&&n| n > 0).count();
    eprintln!(
        "[mc] shannon-split: compiling a cofactor on {} \
         ({} vars, eff support {}/{}, {} clauses)",
        PORTFOLIO_VTREE_SPEC,
        form.num_vars(),
        eff_support,
        form.num_vars(),
        form.clauses().len(),
    );
    count_attempt(run, portfolio_attempt(run, form, run.deadline, None)?, run.deadline)
}

/// Whether the no-SBVA retry may run behind the primary compile: the input is
/// coloring-like, as vitri's structure profile reads it, and vitri's SBVA
/// policy is on. Says so on stderr when it is.
pub(super) fn arm_no_sbva_retry(config: &Config) -> bool {
    let Some(profile) = config.source_structure_profile.filter(|profile| profile.coloring_like) else {
        return false;
    };
    if config.vitri.arjun.sbva != vitri::preprocess::ArjunSbva::On {
        return false;
    }
    eprintln!(
        "[mc] no-sbva-retry: armed (coloring-like input: occ_cv={:.4} width_cv={:.4})",
        profile.var_occurrence_cv, profile.clause_width_cv,
    );
    true
}

/// First fallback: re-reduce the same formula with SBVA off and recompile,
/// inside this stage's own reserve.
///
/// The primary compile keeps SBVA, which helps broadly; this stage recovers
/// the class where SBVA is the reason the primary compile fails: SBVA
/// rewrites the input clause set into a shape whose diagram grows until
/// recovery. Re-reducing the same `compile_formula` with SBVA off, asking
/// vitri for its hypergraph construction, and re-entering the same compile
/// path answers it.
///
/// Engagement (all three, else the stage does not run):
///  - the primary compile failed — this stage runs only among the fallbacks
///    of [`count_primary`], which run only then;
///  - [`arm_no_sbva_retry`] armed it: the input is coloring-like and vitri's
///    SBVA policy is on;
///  - SBVA ran in the primary reduction, as vitri's stage report says.
///
/// The reduction is a retry of the same frontend session with SBVA off and the
/// hypergraph vtree. vitri owns the clause-growth discard and reports it as a
/// decline, which leaves the formula to the next fallback. A kept reduction
/// becomes a compile attempt through `attempt_from_vitri_build`, like the
/// primary one.
///
/// Ordering: this runs before the unreduced retry below. It is the targeted
/// stage — gated on a structure signal that fires on a narrow class — while
/// the unreduced retry is the generic catch-all, and the no-SBVA route is the
/// cheaper of the two.
fn no_sbva_retry(
    run: &Run<'_>,
    compile_formula: &CnfFormula,
    session: &vitri::bundle::FrontendSession<'_>,
) -> Option<Counted> {
    let program_deadline = run.deadline;
    let now = Instant::now();
    let reserve = no_sbva_retry_window(run.config.time_limit_ms, program_deadline, now)?;
    // The stage's own wall, never past the program deadline. Bounds the
    // re-reduce, the vtree construction and the compile alike, so a stage
    // advertised as "up to Ns" cannot overrun into the tail the unreduced
    // retry and Shannon split still need.
    let stage_deadline = crate::budget::not_past(now + reserve, program_deadline);
    eprintln!(
        "[mc] no-sbva-retry: re-reducing without SBVA and recompiling on {} \
         for up to {}s before support enumeration, the unreduced retry and Shannon split",
        RETRY_VTREE_SPEC,
        reserve.as_secs(),
    );
    // Reduce budget: the front end's own Arjun budget for the run's time
    // limit (`ArjunBudget::derived_duration`), capped at half this stage's
    // window so the reduce cannot consume the compile's share — on the class
    // this stage is for, the reduce is the smaller half of the route.
    let arjun_budget = vitri::config::ArjunBudget::derived_duration(run.config.time_limit_ms);
    let reduce_budget_ms = (arjun_budget.as_millis() as u64).min(reserve.as_millis() as u64 / 2);
    let t_reduce = Instant::now();
    let budget = vitri::bundle::RetryBudget::new(
        stage_deadline,
        std::time::Duration::from_millis(reduce_budget_ms),
    )
    .ok()?;
    let retry = vitri::bundle::FrontendRetryConfig {
        arjun_sbva: Some(vitri::preprocess::ArjunSbva::Off),
        vtree_spec: Some(RETRY_VTREE_SPEC.to_owned()),
    };
    let retried = match session.retry(budget, &retry) {
        Ok(Some(retried)) => retried,
        Ok(None) => {
            eprintln!("[mc] no-sbva-retry: vitri's frontend declined — leaving the formula to the next fallback");
            return None;
        }
        Err(error) => {
            eprintln!(
                "[mc] no-sbva-retry: vitri's frontend failed ({error}) — leaving the formula to the next fallback"
            );
            return None;
        }
    };
    let crate::frontend::McFrontendAttempt { preprocessed: ar, vtree } =
        crate::frontend::mc_frontend_attempt(retried, &run.config.vtree_time);
    let arjun_pow2 = ar.count_lift.arjun_pow2;
    eprintln!(
        "[mc] no-sbva-retry: arjun reduced in {}ms — {}v/{}c → {}v/{}c, lift=2^{}",
        ar.telemetry.arjun_ms.unwrap_or(ar.telemetry.total_ms),
        compile_formula.num_vars(),
        compile_formula.clauses().len(),
        ar.reduced.num_vars(),
        ar.reduced.clauses().len(),
        arjun_pow2,
    );
    let built = match vtree {
        // A refuted retry has answered the instance: the count is 0, and no
        // lift can move it, so nothing is built or compiled on top of it.
        crate::frontend::FrontendVtree::Refuted => {
            eprintln!("[mc] no-sbva-retry: arjun refuted the formula");
            return Some((num_bigint::BigUint::from(0u32), 0));
        }
        // Full-resolve short-circuit: the answer is the reduction's lift
        // alone, folded here as any count of a reduction is.
        crate::frontend::FrontendVtree::FullyResolved => {
            eprintln!("[mc] no-sbva-retry: arjun resolved every variable");
            return Some((num_bigint::BigUint::from(1u32) << arjun_pow2, 0));
        }
        // `StageOutcome::Ran` means vitri's own NotSmaller gate kept this
        // reduction. A discarded result returned above, so there is no second
        // clause-growth decision here.
        crate::frontend::FrontendVtree::Built(built) => built.bind(run.eng),
    };
    let attempt = attempt_from_vitri_build(ar.reduced, built, arjun_pow2);
    // The attempt carries this reduction's lift, which its compile folds.
    let counted = count_attempt(run, attempt, Some(stage_deadline))?;
    eprintln!(
        "[mc] no-sbva-retry: counted in {}ms",
        t_reduce.elapsed().as_millis(),
    );
    Some(counted)
}

/// The portfolio entry the stall recompile builds its tree from: goatd's
/// refinement schedule on the primal graph, an entry the default portfolio
/// leaves out. On the reductions whose primary compile stalls, it is the entry
/// that most often compiled when each was forced alone.
const STALL_RECOMPILE_ENTRY: &str = "goatd-primal";

/// The stall recompile: the primary compile gave up on one internal step that
/// ran past its own wall ([`super::windows::stall_rule`]) with the run's time
/// still left, so compile the same reduction `ar` again, on a tree the
/// portfolio builds for it from [`STALL_RECOMPILE_ENTRY`] alone, to the run's
/// deadline.
///
/// The stalled step is at a node of the primary's tree far from its root; a
/// tree of another construction can split the formula elsewhere, and the
/// reduction is already paid for, so this runs before the fallbacks, which
/// re-reduce or drop it. The tree is named on stderr and in the run's
/// `started_on` as the compile starts, as the primary's is. The count folds
/// the reduction's lift, as the primary's does. No per-step wall: a recompile
/// that stalls too runs on to the deadline. `None` when it answers nothing;
/// the caller then takes the primary-failure path.
fn stall_recompile(run: &Run<'_>, ar: &crate::frontend::record::ArjunReduction) -> Option<Counted> {
    // The stalled compile's scratch and dirty pages go back first, as before
    // any compile that follows a failed one.
    release_failed_compile(run.eng);
    let attempt = portfolio_attempt(run, &ar.formula, run.deadline, Some(STALL_RECOMPILE_ENTRY));
    let counted = attempt.and_then(|attempt| {
        run.announce_tree("[mc] stall-recompile: compiling the reduction", " on ");
        count_attempt(run, CompileAttempt { lift_pow2: ar.lift_pow2, ..attempt }, run.deadline)
    });
    if counted.is_none() {
        eprintln!("[mc] stall-recompile: gave up");
    }
    counted
}

/// Third fallback: compile the unreduced formula once. The primary compile's
/// failure — an OOM panic, or an attempt that answered nothing by its
/// deadline — is intercepted at depth 0 (before Shannon split spends the
/// wall) and the unreduced `compile_formula` is compiled once on a bounded
/// portfolio attempt. Arjun's reduction can make the diagram grow until it
/// runs out of memory while the unreduced formula compiles cheaply (the class
/// where the reduction hurts). Shannon split does not help that class (each
/// cofactor is reduced again and grows the same way), so this retry runs
/// before Shannon split. The two narrower fallbacks run ahead of it. Bounded
/// to the `unreduced_retry_window` window so an attempt that does not answer
/// falls through to Shannon split, which answers other classes.
///
/// It compiles `compile_formula` itself, so there is no lift to fold. Fires
/// whether or not `primary_deadline` armed: at short windows the absolute
/// reserve is inert, and the unreduced retry still has to run ahead of
/// Shannon split rather than after it.
fn unreduced_retry(
    run: &Run<'_>,
    form: &CnfFormula,
    primary_deadline: Option<Instant>,
) -> Option<Counted> {
    let program_deadline = run.deadline;
    let now = Instant::now();
    let reserve = unreduced_retry_window(primary_deadline, program_deadline, now)?;
    let retry_deadline = Some(crate::budget::not_past(now + reserve, program_deadline));
    if let Some(primary) = primary_deadline {
        // The window, and the two numbers it is drawn from: the tail the
        // primary deadline reserved at the top of the run, and the wall the
        // run actually has left here. They differ by however early the
        // primary compile quit, which is the whole reason this stage stopped
        // getting the wall it needs, so a reader chasing a lost instance can
        // see the split without reconstructing it from timestamps.
        eprintln!(
            "[mc] unreduced-retry: compiling the unreduced formula on {} for up \
             to {}s before Shannon split (reserved {}s, {}s left)",
            PORTFOLIO_VTREE_SPEC,
            reserve.as_secs(),
            program_deadline.map_or(0, |program| program.saturating_duration_since(primary).as_secs()),
            program_deadline.map_or(0, |program| program.saturating_duration_since(now).as_secs()),
        );
    } else {
        eprintln!(
            "[mc] unreduced-retry: compiling the unreduced formula on {} for up \
             to {}s before Shannon split",
            PORTFOLIO_VTREE_SPEC,
            reserve.as_secs()
        );
    }
    // `retry_deadline` — the reserve window this retry just advertised —
    // bounds its vtree construction too, not the (looser) program deadline.
    let attempt = portfolio_attempt(run, form, retry_deadline, None)?;
    let counted = count_attempt(run, attempt, retry_deadline)?;
    eprintln!("[mc] unreduced-retry: counted");
    Some(counted)
}

/// The kept reduction: compile it on the vtree vitri built for it, under
/// [`count_with_recovery`], with the fallbacks behind it and Shannon split
/// behind them. `None` when nothing answered; the stage that gave up has said
/// so.
///
/// vitri's own clause-growth policy already discarded the reductions it did not
/// want (they arrive at `count_without_reduction`), so this crate runs no second
/// gate.
///
/// Every step that answers folds its own lift (the correction invariant, in
/// the `mc` module doc): a compile attempt through its `lift_pow2`, the
/// support-enum stage through `ar.lift_pow2`. The count returned is in
/// `compile_formula`'s space.
///
/// The primary compile runs under `stall_rule`, the run's per-step wall
/// (`None`, no wall). A primary that gives up on it, with the run's time
/// left, goes to the [`stall_recompile`] before the fallbacks.
pub(super) fn count_primary(
    // The run; its wall bounds the whole chain, Shannon cascade included.
    run: &Run<'_>,
    compile_formula: &CnfFormula,
    // The Arjun reduction the primary compile runs on, also handed to the
    // support-enum stage among the fallbacks. The stage's own preconditions
    // decide.
    ar: crate::frontend::record::ArjunReduction,
    vtree: crate::vtree::VtreeBuild,
    // The same vitri session that produced the reduction when the no-SBVA
    // retry is a candidate, `None` when it is not; checkpointed retries stay
    // anchored to it instead of replaying preprocessing. Candidacy is derived
    // only from vitri's structure profile of the input and its stage report.
    // vitri rechecks eligibility when the retry is requested.
    no_sbva_session: Option<&vitri::bundle::FrontendSession<'_>>,
    stall_rule: Option<StallRule>,
) -> Option<Recovered> {
    // The primary (Arjun-reduced) compile's own wall: `program_deadline −
    // tail`, so a compile that would otherwise grind to the program end is
    // cut with the tail still unspent. Being cut is an ordinary primary
    // failure and enters the fallbacks like any other. The no-SBVA retry and
    // the unreduced retry are sized against that tail
    // (`no_sbva_retry_window`, `unreduced_retry_window`); the unreduced
    // retry then compiles the unreduced formula on exactly that tail
    // (`unreduced_retry_window`'s `program - primary` case). This answers the class
    // where the unreduced formula compiles and the reduction does not,
    // without compiling both every time. The tail is `UNREDUCED_RETRY_RESERVE`
    // (enough for the unreduced retry that class needs while leaving Shannon
    // split behind it what it needs), plus the no-SBVA retry's want when that
    // stage is armed. It engages only when the remaining wall exceeds twice
    // the tail; below that the unreduced retry still runs before Shannon
    // split, on `unreduced_retry_window`'s remaining/4. `None` = off: the
    // primary compile runs to `program_deadline`.
    let program_deadline = run.deadline;
    let primary_deadline = {
        let mut tail = UNREDUCED_RETRY_RESERVE;
        if no_sbva_session.is_some() && let Some(total_ms) = run.config.time_limit_ms {
            tail += no_sbva_retry_want(total_ms);
        }
        program_deadline.and_then(|d| {
            let remaining = d.saturating_duration_since(Instant::now());
            (remaining > tail * 2).then(|| d - tail)
        })
    };
    // The primary compile: the Arjun reduction on the vtree vitri built for
    // it, stopping at the reserved-tail deadline when one is set, and each of
    // its internal steps the stall rule bounds at its own wall. A stall, a
    // step stopped on that wall, which ends before the primary's deadline,
    // hands the reduction to the stall recompile while the run's time lasts.
    let primary = attempt_from_vitri_build(ar.formula.clone(), vtree, ar.lift_pow2);
    let stall_watch = stall_rule.map(StallWatch::new);
    let first = || {
        let watched = Run { compile: CompileRun { stall_watch: stall_watch.as_ref(), ..run.compile }, ..*run };
        if let Some(counted) = count_attempt(&watched, primary, primary_deadline.or(program_deadline)) {
            return Some((counted, AnsweredBy::Compile));
        }
        if let Some(watch) = &stall_watch
            && let Some(stall) = watch.stall()
            && program_deadline.is_none_or(|d| Instant::now() < d)
        {
            eprintln!(
                "[mc] compile: the step at vtree node {} (internal step {}/{}) ran {:.1}s, past its \
                 {}s wall — giving the primary compile up",
                stall.node,
                stall.k,
                stall.total,
                stall.ran.as_secs_f64(),
                watch.rule().wall.as_secs(),
            );
            if let Some(counted) = stall_recompile(run, &ar) {
                return Some((counted, AnsweredBy::StallRecompile));
            }
        }
        if primary_deadline.is_some_and(|d| Instant::now() >= d) {
            eprintln!("[mc] compile: reached its deadline — the fallbacks run on the reserved time");
        }
        None
    };
    // The fallbacks, stopping at the first that answers; each skips itself
    // when it has nothing to do.
    let fallbacks = || {
        no_sbva_session
            .and_then(|session| no_sbva_retry(run, compile_formula, session))
            .map(|counted| (counted, AnsweredBy::NoSbvaRetry))
            // Count the Arjun-reduced formula by enumerating the feasible
            // assignments to Arjun's own minimized independent support
            // (`support_enum.rs` has soundness and budget).
            .or_else(|| {
                support_enum_stage(
                    &ar.formula,
                    &ar.support,
                    ar.lift_pow2,
                    "kept reduction",
                    program_deadline,
                )
                .map(|count| ((count, 0), AnsweredBy::SupportEnumeration))
            })
            .or_else(|| {
                unreduced_retry(run, compile_formula, primary_deadline)
                    .map(|counted| (counted, AnsweredBy::UnreducedRetry))
            })
    };
    // The tree is named before the compile starts, as the plain compile does,
    // so a run that gives up still says which tree it was on.
    run.announce_tree("[mc] compile: start", " on ");
    count_with_recovery(run, compile_formula, first, Some(&fallbacks), count_cofactor)
}

#[cfg(test)]
#[path = "tests/cascade.rs"]
mod tests;
