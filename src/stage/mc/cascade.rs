//! The primary compile and the fallbacks that run when it fails.
//!
//! The primary compile runs first. If it fails, the fallbacks run — the
//! bisection retry, the no-SBVA re-reduce, support enumeration, the
//! unreduced retry — and then Shannon split, as [`count_with_recovery`]
//! orders them.
//! Each function below is one step of that order and says what it decides.
//! The monotone reduction is the fallback of the count without a reduction
//! instead, on a formula Arjun skipped as monotone. Every step that answers
//! folds its own `2^k` multiplier before returning, so the count leaving this
//! module is always in the space of the formula it was handed.

use std::time::Instant;

use vitri::cnf::CnfFormula;
use crate::compile::release_failed_compile;
use crate::config::Config;
use crate::output::AnsweredBy;
use crate::stage::{PORTFOLIO_VTREE_SPEC, RETRY_VTREE_SPEC, Run, bisect_retry_applies};

use super::windows::{
    UNREDUCED_RETRY_RESERVE, monotone_reduction_window, no_sbva_retry_want, no_sbva_retry_window,
    unreduced_retry_window,
};
use super::attempt::{attempt_from_vitri_build, attempt_on, count_attempt};
use super::recovery::{Counted, Recovered, count_with_recovery};
use super::support_enum::{EnumOutcome, ProbeMode, support_enum_stage};

/// A Shannon cofactor's count, in both cascades, taken the way the input's
/// is: the compile of its reduction, then, when that gives up with the run's
/// wall left, one compile of the cofactor as it stands on the portfolio's tree
/// for it, as the unreduced retry compiles the input. The two trees cut the
/// cofactor in different places, and either can fit where the other ran out
/// of memory. The run's wall bounds both. `None` when neither answers.
pub(super) fn count_cofactor(run: &Run<'_>, form: &CnfFormula) -> Option<Counted> {
    if let Some(counted) = count_reduced_cofactor(run, form) {
        return Some(counted);
    }
    if crate::budget::deadline_passed(run.deadline) {
        return None;
    }
    // The failed compile's scratch and dirty pages go back first, as before
    // any compile that follows a failed one.
    release_failed_compile(run.eng);
    eprintln!(
        "[mc] shannon-split: compiling the cofactor as it stands: {}v/{}c",
        form.num_vars(),
        form.clauses().len(),
    );
    count_attempt(run, attempt_on(run, form, PORTFOLIO_VTREE_SPEC, 0, run.deadline)?, run.deadline)
}

/// A cofactor through the run's frontend, as the input went: vitri's
/// preprocessing, then the compile on the vtree vitri pairs with its
/// reduction, with no implied clauses, the reduction's lift folded, so the
/// count is in the cofactor's space. A cofactor the preprocessing settles needs no compile; a frontend
/// that fails gives up with `None`, as a compile that gives up does.
fn count_reduced_cofactor(run: &Run<'_>, form: &CnfFormula) -> Option<Counted> {
    let counted = crate::frontend::with_mc_frontend(
        form,
        run.config,
        run.deadline,
        crate::frontend::TreeFor::Recovery,
        |attempt, _| {
            let prepared = attempt.preprocessed;
            let lift_pow2 = prepared.count_lift.total_pow2();
            eprintln!(
                "[mc] shannon-split: compiling a cofactor: {}v/{}c reduced to {}v/{}c, lift=2^{lift_pow2}",
                form.num_vars(),
                form.clauses().len(),
                prepared.reduced.num_vars(),
                prepared.reduced.clauses().len(),
            );
            match attempt.vtree {
                crate::frontend::FrontendVtree::Built(built) => {
                    let attempt = attempt_from_vitri_build(prepared.reduced, built.bind(run), lift_pow2);
                    count_attempt(run, attempt, run.deadline)
                }
                crate::frontend::FrontendVtree::FullyResolved => Some((num_bigint::BigUint::from(1u32) << lift_pow2, 0)),
                crate::frontend::FrontendVtree::Refuted => Some((num_bigint::BigUint::from(0u32), 0)),
            }
        },
    );
    counted.unwrap_or_else(|error| {
        eprintln!("[mc] shannon-split: vitri's frontend failed on a cofactor: {error}");
        None
    })
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

struct RetryLog<'a> {
    tag: &'a str,
    declined: &'a str,
    next: &'a str,
}

/// Retry `session` under `retry` inside `(window, stage_deadline)`: cap Arjun
/// at half `window` so the reduce cannot consume the compile's share, run the
/// retry, log the reduction summary on stderr, answer early when the retry
/// refuted or fully resolved `formula`, and otherwise bind its vtree to
/// `run.eng`.
fn retry_frontend_session(
    run: &Run<'_>,
    formula: &CnfFormula,
    session: &vitri::bundle::FrontendSession<'_>,
    window: std::time::Duration,
    stage_deadline: Instant,
    retry: &vitri::bundle::FrontendRetryConfig,
    log: RetryLog<'_>,
) -> Option<std::ops::ControlFlow<Counted, (vitri::bundle::PreprocessBundle, crate::vtree::VtreeBuild)>> {
    let arjun_budget = vitri::config::ArjunBudget::derived_duration(run.config.time_limit_ms).min(window / 2);
    let budget = vitri::bundle::RetryBudget::new(stage_deadline, arjun_budget).ok()?;
    let retried = match run.config.vtree_time.count(|| session.retry(budget, retry)) {
        Ok(Some(retried)) => retried,
        Ok(None) => {
            eprintln!("{}: {} — {}", log.tag, log.declined, log.next);
            return None;
        }
        Err(error) => {
            eprintln!("{}: vitri's frontend failed ({error}) — {}", log.tag, log.next);
            return None;
        }
    };
    let tag = log.tag;
    let crate::frontend::McFrontendAttempt { preprocessed, vtree } =
        crate::frontend::mc_frontend_attempt(retried, &run.config.vtree_time);
    let arjun_pow2 = preprocessed.count_lift.arjun_pow2;
    eprintln!(
        "{tag}: arjun reduced in {}ms — {}v/{}c → {}v/{}c, lift=2^{}",
        preprocessed.telemetry.arjun_ms.unwrap_or(preprocessed.telemetry.total_ms),
        formula.num_vars(),
        formula.clauses().len(),
        preprocessed.reduced.num_vars(),
        preprocessed.reduced.clauses().len(),
        arjun_pow2,
    );
    Some(match vtree {
        // A refuted retry has answered the instance: the count is 0, and no
        // lift can move it, so nothing is built or compiled on top of it.
        crate::frontend::FrontendVtree::Refuted => {
            eprintln!("{tag}: arjun refuted the formula");
            std::ops::ControlFlow::Break((num_bigint::BigUint::from(0u32), 0))
        }
        // Full-resolve short-circuit: the answer is the reduction's lift
        // alone, folded here as any count of a reduction is.
        crate::frontend::FrontendVtree::FullyResolved => {
            eprintln!("{tag}: arjun resolved every variable");
            std::ops::ControlFlow::Break((num_bigint::BigUint::from(1u32) << arjun_pow2, 0))
        }
        // `StageOutcome::Ran` means vitri's own NotSmaller gate kept this
        // reduction. A discarded result returned before this match, so there
        // is no second clause-growth decision here.
        crate::frontend::FrontendVtree::Built(built) => {
            std::ops::ControlFlow::Continue((preprocessed, built.bind(run)))
        }
    })
}

/// The input formula and the simplify chain's lift, for the no-SBVA retry of
/// [`count_primary`], which re-reduces the input with vitri's DVE off.
pub(super) struct NoSbvaRoute<'a> {
    pub(super) input: &'a CnfFormula,
    pub(super) simplify_pow2: u32,
}

/// The no-SBVA retry of [`count_primary`]: as [`no_sbva_retry`], but from the
/// run's input through a fresh frontend with vitri's DVE and gate detection
/// off, SBVA off and the bisection tree. On a formula at the edge of the
/// memory cap, the variables DVE takes out change Arjun's output and the tree
/// enough to decide whether the compile fits, and the route without DVE is
/// the other draw. The count comes back in the input's space and is returned
/// in the simplified formula's, as the other fallbacks of [`count_primary`]
/// are.
fn no_sbva_retry_without_dve(run: &Run<'_>, route: &NoSbvaRoute<'_>) -> Option<Counted> {
    let program_deadline = run.deadline;
    let now = Instant::now();
    let reserve = no_sbva_retry_window(run.config.time_limit_ms, program_deadline, now)?;
    let stage_deadline = crate::budget::not_past(now + reserve, program_deadline);
    eprintln!(
        "[mc] no-sbva-retry: re-reducing the input without DVE or SBVA and recompiling on {} \
         for up to {}s before the fallbacks after it and Shannon split",
        RETRY_VTREE_SPEC,
        reserve.as_secs(),
    );
    let t_reduce = Instant::now();
    let arjun_budget = vitri::config::ArjunBudget::derived_duration(run.config.time_limit_ms).min(reserve / 2);
    let input = route.input;
    let counted = crate::frontend::with_mc_frontend_adjusted(
        input,
        run.config,
        Some(stage_deadline),
        crate::frontend::TreeFor::Recovery,
        |config| {
            // vitri refuses a simplify policy under a disabled simplify stage.
            if config.stages.simplify {
                config.simplify.dve = None;
                config.simplify.detect_gates = false;
            }
            config.arjun.sbva = vitri::preprocess::ArjunSbva::Off;
            config.arjun_budget = vitri::config::ArjunBudget::Exact(arjun_budget);
            config.vtree_spec = RETRY_VTREE_SPEC.to_owned();
        },
        |attempt, _| {
            let prepared = attempt.preprocessed;
            let lift_pow2 = prepared.count_lift.total_pow2();
            eprintln!(
                "[mc] no-sbva-retry: arjun reduced in {}ms — {}v/{}c → {}v/{}c, lift=2^{lift_pow2}",
                prepared.telemetry.arjun_ms.unwrap_or(prepared.telemetry.total_ms),
                input.num_vars(),
                input.clauses().len(),
                prepared.reduced.num_vars(),
                prepared.reduced.clauses().len(),
            );
            match attempt.vtree {
                crate::frontend::FrontendVtree::Built(built) => {
                    let attempt = attempt_from_vitri_build(prepared.reduced, built.bind(run), lift_pow2);
                    count_attempt(run, attempt, Some(stage_deadline))
                }
                crate::frontend::FrontendVtree::FullyResolved => Some((num_bigint::BigUint::from(1u32) << lift_pow2, 0)),
                crate::frontend::FrontendVtree::Refuted => Some((num_bigint::BigUint::from(0u32), 0)),
            }
        },
    );
    let (count, peak_pairs) = counted.unwrap_or_else(|error| {
        eprintln!("[mc] no-sbva-retry: vitri's frontend failed ({error}) — leaving the formula to the next fallback");
        None
    })?;
    // The input's count is the simplified formula's times 2^simplify_pow2, so
    // a count with fewer trailing zeros is a wrong count, never returned.
    if count.trailing_zeros().is_some_and(|zeros| zeros < u64::from(route.simplify_pow2)) {
        eprintln!("[mc] no-sbva-retry: the count is not a multiple of the simplify lift — discarded");
        return None;
    }
    eprintln!("[mc] no-sbva-retry: counted in {}ms", t_reduce.elapsed().as_millis());
    Some((count >> route.simplify_pow2, peak_pairs))
}

/// Second fallback: re-reduce the same formula with SBVA off and recompile,
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
///
/// The [`monotone_reduction`] runs it too, with `reduce_monotone` set, so the
/// retry asks Arjun for the reduction it skipped on that formula.
fn no_sbva_retry(
    run: &Run<'_>,
    compile_formula: &CnfFormula,
    session: &vitri::bundle::FrontendSession<'_>,
    reduce_monotone: bool,
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
         for up to {}s before the fallbacks after it and Shannon split",
        RETRY_VTREE_SPEC,
        reserve.as_secs(),
    );
    let t_reduce = Instant::now();
    let retry = vitri::bundle::FrontendRetryConfig {
        arjun_sbva: Some(vitri::preprocess::ArjunSbva::Off),
        vtree_spec: Some(RETRY_VTREE_SPEC.to_owned()),
        arjun_skip_monotone: reduce_monotone.then_some(false),
    };
    let log = RetryLog {
        tag: "[mc] no-sbva-retry",
        declined: "vitri's frontend declined",
        next: "leaving the formula to the next fallback",
    };
    let (ar, built) = match retry_frontend_session(run, compile_formula, session, reserve, stage_deadline, &retry, log)?
    {
        std::ops::ControlFlow::Break(counted) => return Some(counted),
        std::ops::ControlFlow::Continue(ready) => ready,
    };
    let attempt = attempt_from_vitri_build(ar.reduced, built, ar.count_lift.arjun_pow2);
    // The attempt carries this reduction's lift, which its compile folds.
    let counted = count_attempt(run, attempt, Some(stage_deadline))?;
    eprintln!(
        "[mc] no-sbva-retry: counted in {}ms",
        t_reduce.elapsed().as_millis(),
    );
    Some(counted)
}

/// The fallback of the count without a reduction when vitri skipped Arjun on
/// `formula` because it is monotone ([`vitri::bundle::SkipReason::Monotone`]):
/// the compile of the formula itself has failed, so ask the frontend session
/// for the reduction it skipped and count it the way a kept reduction is
/// counted. Its compile on the tree vitri built for it comes first, unless it
/// is no smaller than `formula`, then the no-SBVA retry when `no_sbva_armed`
/// and SBVA ran, then support enumeration over its independent support.
/// `None` when none answers; Shannon split follows.
///
/// The skip is the better start on most monotone formulas, whose unreduced
/// compile answers sooner than Arjun would finish. On some, only the
/// reduction's compile finishes, and this answers them. It takes the place of
/// the support hunt the count without a reduction runs on other formulas, whose
/// fresh Arjun pass skips this one too.
///
/// The whole stage runs inside [`monotone_reduction_window`], which leaves
/// Shannon split its reserve, and its compile stops early enough to leave the
/// no-SBVA retry its window. The retry starts from the session's simplify
/// checkpoint, so the reduction is of `formula` itself and only its Arjun
/// lift is folded.
pub(super) fn monotone_reduction(
    run: &Run<'_>,
    formula: &CnfFormula,
    session: &vitri::bundle::FrontendSession<'_>,
    no_sbva_armed: bool,
) -> Option<(Counted, AnsweredBy)> {
    let now = Instant::now();
    let Some(window) = monotone_reduction_window(run.deadline, now) else {
        eprintln!("[mc] monotone-reduction: skipped — too little time is left to reduce and compile");
        return None;
    };
    let stage_deadline = crate::budget::not_past(now + window, run.deadline);
    // Every step below, the no-SBVA retry included, stops at the stage's own
    // wall.
    let run = &Run { deadline: Some(stage_deadline), ..*run };
    eprintln!(
        "[mc] monotone-reduction: Arjun skipped the monotone formula — reducing it and compiling \
         the reduction for up to {}s before Shannon split",
        window.as_secs(),
    );
    let retry = vitri::bundle::FrontendRetryConfig {
        arjun_skip_monotone: Some(false),
        ..vitri::bundle::FrontendRetryConfig::default()
    };
    let log = RetryLog {
        tag: "[mc] monotone-reduction",
        declined: "Arjun kept no reduction",
        next: "Shannon split next",
    };
    let (bundle, built) = match retry_frontend_session(run, formula, session, window, stage_deadline, &retry, log)? {
        std::ops::ControlFlow::Break(counted) => return Some((counted, AnsweredBy::MonotoneReduction)),
        std::ops::ControlFlow::Continue(ready) => ready,
    };
    let no_sbva = no_sbva_armed && matches!(bundle.stages.sbva, Some(vitri::bundle::StageOutcome::Ran));
    let ar = crate::frontend::record::arjun_reduction(bundle)
        .expect("a kept plain-MC Arjun result carries its support");
    // A reduction no smaller than the formula is the formula whose compile
    // just failed, and the portfolio picks its tree again; compiling it would
    // repeat that compile.
    if ar.formula.num_vars() >= formula.num_vars() && ar.formula.clauses().len() >= formula.clauses().len() {
        eprintln!("[mc] monotone-reduction: the reduction is no smaller — not compiling it again");
    } else {
        // The no-SBVA retry's window comes out of the compile's when it is armed.
        let compile_deadline = match run.config.time_limit_ms {
            Some(total_ms) if no_sbva => stage_deadline
                .checked_sub(no_sbva_retry_want(total_ms))
                .filter(|&at| at > Instant::now())
                .unwrap_or(stage_deadline),
            _ => stage_deadline,
        };
        let attempt = attempt_from_vitri_build(ar.formula.clone(), built, ar.lift_pow2);
        if let Some(counted) = count_attempt(run, attempt, Some(compile_deadline)) {
            return Some((counted, AnsweredBy::MonotoneReduction));
        }
        eprintln!("[mc] monotone-reduction: the compile of the reduction gave up");
        release_failed_compile(run.eng);
    }
    if no_sbva && let Some(counted) = no_sbva_retry(run, formula, session, true) {
        return Some((counted, AnsweredBy::NoSbvaRetry));
    }
    support_enum_stage(
        &ar.formula,
        &ar.support,
        ar.lift_pow2,
        "monotone reduction",
        run.deadline,
        &run.config.vtree_time,
        ProbeMode::Gate,
    )
    .count()
    .map(|count| ((count, 0), AnsweredBy::SupportEnumeration))
}

/// First fallback: compile the kept reduction once more on vitri's
/// hypergraph bisection, [`RETRY_VTREE_SPEC`], as the plain compile does when
/// its compile on the portfolio's vtree runs out of memory, and when
/// [`bisect_retry_applies`] says so: `pick` is the portfolio's winner, and
/// the primary compile stopped before `primary_deadline`, the run's wall
/// when that is unarmed.
///
/// The portfolio scores its candidates without compiling on them, and the
/// candidates it ranks close tend to run out of memory together, while the
/// bisection, which keeps each clause's variables together, compiles some of
/// those formulas in a fraction of the memory. The retry builds its
/// tree and compiles to the primary compile's deadline, and when that is armed
/// for at most a quarter of the declared wall: at a long limit the fallbacks
/// behind it keep the tail that deadline reserved and whatever the quarter
/// leaves of the time before it, and at a short one they get what a retry
/// that runs out of memory leaves.
fn bisect_retry(
    run: &Run<'_>,
    ar: &crate::frontend::record::ArjunReduction,
    pick: Option<&str>,
    primary_deadline: Option<Instant>,
) -> Option<Counted> {
    let now = Instant::now();
    // With the primary deadline armed, the retry gets at most the no-SBVA
    // retry's window, a quarter of the declared wall, so a retry that does
    // not run out of memory still leaves the stages behind it their time.
    let cap = run.config.time_limit_ms.map(|total_ms| now + no_sbva_retry_want(total_ms));
    let deadline = match (primary_deadline, cap) {
        (Some(primary), Some(cap)) => Some(primary.min(cap)),
        (primary, _) => primary.or(run.deadline),
    };
    if !bisect_retry_applies(run.config.explicit_vtree(), pick, deadline, now) {
        return None;
    }
    eprintln!(
        "[mc] bisect-retry: the compile on {} ran out of memory; compiling the reduction once more on \
         {RETRY_VTREE_SPEC} ({})",
        pick.unwrap_or(run.config.vtree_spec()),
        deadline.map_or_else(
            || "no time limit".to_owned(),
            |deadline| format!("{}s left", deadline.saturating_duration_since(now).as_secs()),
        ),
    );
    let attempt = attempt_on(run, &ar.formula, RETRY_VTREE_SPEC, ar.lift_pow2, deadline)?;
    let counted = count_attempt(run, attempt, deadline)?;
    eprintln!("[mc] bisect-retry: counted in {}ms", now.elapsed().as_millis());
    Some(counted)
}

/// Fourth fallback: compile the unreduced formula once. The primary compile's
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
    let attempt = attempt_on(run, form, PORTFOLIO_VTREE_SPEC, 0, retry_deadline)?;
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
pub(super) fn count_primary(
    run: &Run<'_>,
    compile_formula: &CnfFormula,
    ar: crate::frontend::record::ArjunReduction,
    vtree: crate::vtree::VtreeBuild,
    no_sbva: Option<NoSbvaRoute<'_>>,
) -> Option<Recovered> {
    // The primary compile's own deadline (`program_deadline - tail`), armed when
    // the remaining wall exceeds twice the reserved tail (`UNREDUCED_RETRY_RESERVE`
    // plus the no-SBVA retry's want when `no_sbva` is set).
    let mut tail = UNREDUCED_RETRY_RESERVE;
    if no_sbva.is_some() && let Some(total_ms) = run.config.time_limit_ms {
        tail += no_sbva_retry_want(total_ms);
    }
    let primary_deadline = run.deadline.and_then(|d| {
        let remaining = d.saturating_duration_since(Instant::now());
        (remaining > tail * 2).then(|| d - tail)
    });
    let first = || {
        let primary = attempt_from_vitri_build(ar.formula.clone(), vtree, ar.lift_pow2);
        let counted = count_attempt(run, primary, primary_deadline.or(run.deadline));
        if counted.is_none() && primary_deadline.is_some_and(|d| Instant::now() >= d) {
            eprintln!("[mc] compile: reached its deadline — the fallbacks run on the reserved time");
        }
        counted
    };
    // The tree is named before the compile starts, as the plain compile does,
    // so a run that gives up still says which tree it was on.
    let pick = run.announce_tree("[mc] compile: start", " on ");
    // The fallbacks, stopping at the first that answers; each skips itself
    // when it has nothing to do.
    let fallbacks = || {
        bisect_retry(run, &ar, pick.as_deref(), primary_deadline)
            .map(|counted| (counted, AnsweredBy::BisectRetry))
            .or_else(|| {
                no_sbva
                    .as_ref()
                    .and_then(|route| no_sbva_retry_without_dve(run, route))
                    .map(|counted| (counted, AnsweredBy::NoSbvaRetry))
            })
            .or_else(|| {
                // Support enumeration, then the unreduced retry; a probe that
                // has disproved the enumeration's estimate puts the retry
                // first and the enumeration on what it leaves.
                let enumerate = |mode| {
                    support_enum_stage(
                        &ar.formula,
                        &ar.support,
                        ar.lift_pow2,
                        "kept reduction",
                        run.deadline,
                        &run.config.vtree_time,
                        mode,
                    )
                };
                let enumerated = |count| ((count, 0), AnsweredBy::SupportEnumeration);
                let unreduced = || {
                    unreduced_retry(run, compile_formula, primary_deadline)
                        .map(|counted| (counted, AnsweredBy::UnreducedRetry))
                };
                match enumerate(ProbeMode::DeferWhenDisproved) {
                    EnumOutcome::Counted(count) => Some(enumerated(count)),
                    EnumOutcome::Stopped => unreduced(),
                    EnumOutcome::Deferred => {
                        unreduced().or_else(|| enumerate(ProbeMode::Resume).count().map(enumerated))
                    }
                }
            })
    };
    count_with_recovery(run, compile_formula, first, Some(&fallbacks), count_cofactor)
}

#[cfg(test)]
#[path = "tests/cascade.rs"]
mod tests;
