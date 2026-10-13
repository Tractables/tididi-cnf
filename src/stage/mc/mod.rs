//! The model count, on its two routes: the cascade from Arjun's reduction to
//! a count, and the streaming compile under an explicit `--vtree`.
//!
//! `count_mc` is the cascade — Vitri's MC frontend, the full-resolve
//! short-circuit, then the primary compile and what follows it — and
//! `run_mc_count` is the driver entry that hands back what it produced;
//! `run_mc_streaming` is the other route's entry. The pieces they compose are
//! `attempt` (what a compile attempt is built from and how it compiles),
//! `windows` (what each fallback may spend), `cascade` (the primary compile
//! and the fallbacks), `support_enum` (the fallback that enumerates Arjun's
//! independent support) and `recovery` (the one order every route counts in:
//! the first compile, the fallbacks, Shannon split). Its stderr lines carry
//! the `[mc]` tag and the stage's record token, and the record's `cascade_ms`
//! is the wall from the first compile to the answer.
//!
//! Every count a step returns is in the space of the formula the step was
//! handed — the correction invariant. A step that counts a reduction of that
//! formula folds the reduction's `2^k` lift into its count before it
//! returns, and no caller folds it again.

use vitri::cnf::CnfFormula;
use crate::output::{AnsweredBy, Attribution, McAnswer, PreprocessReport, Reduction};
use crate::stage::Run;

use attempt::{attempt_from_vitri_build, count_attempt};
use cascade::{arm_no_sbva_retry, count_cofactor, count_primary};
use recovery::{Recovered, count_with_recovery};
use support_enum::{reduce_for_support, support_enum_stage};

mod attempt;
mod cascade;
mod recovery;
mod support_enum;
mod windows;

/// What [`count_mc`] produced — a count and the diagnostics that describe how
/// it was reached.
///
/// The count is in the space of the formula passed to [`count_mc`]. Vitri's
/// simplify and Arjun lifts are both folded exactly once before this value leaves
/// the session-backed frontend.
struct McOutcome {
    count: num_bigint::BigUint,
    peak_pairs: usize,
    /// Wall spent in the compile cascade. `None` where the stage that answered
    /// never entered it.
    cascade_ms: Option<u128>,
    answered_by: AnsweredBy,
    /// What became of Arjun's reduction.
    reduction: Option<Reduction>,
    /// Preprocessing telemetry for the frontend attempt that owns this count,
    /// which the driver prints as it is.
    preprocessing: vitri::bundle::PreprocessTelemetry,
}

/// The model count, from the reduction to a count: the Arjun reduction, the
/// full-resolve short-circuit, then either the primary compile with its
/// fallbacks and Shannon-split recovery ([`count_primary`]) or, when Vitri
/// kept no reduction, the count without it ([`count_without_reduction`]).
/// `None` when nothing answered; the stage that gave up has said so on stderr.
/// It runs the whole front end on the formula it is handed.
fn count_mc(run: &Run<'_>, compile_formula: &CnfFormula) -> Option<McOutcome> {
    let no_sbva_retry_armed = arm_no_sbva_retry(run.config);
    let result = crate::frontend::with_mc_frontend(
        compile_formula,
        run.config,
        run.deadline,
        |attempt, session| {
            let mut bundle = attempt.preprocessed;
            let simplify_pow2 = bundle.count_lift.simplify_pow2;
            let preprocessing = bundle.telemetry;
            let reduction = reduction(bundle.stages.arjun.as_ref());
            // Both vtree-less outcomes end here: a refuted formula counts 0, a
            // fully resolved one the lift alone.
            let primary_vtree = match attempt.vtree {
                crate::frontend::FrontendVtree::Built(built) => built.bind(run.eng),
                _ => {
                    let count = if bundle.record.unsat {
                        num_bigint::BigUint::from(0u32)
                    } else {
                        num_bigint::BigUint::from(1u32) << bundle.count_lift.total_pow2()
                    };
                    return Some(McOutcome {
                        count,
                        peak_pairs: 0,
                        cascade_ms: None,
                        answered_by: AnsweredBy::Preprocessing,
                        reduction,
                        preprocessing,
                    });
                }
            };
            let recovered = if matches!(bundle.stages.arjun, Some(vitri::bundle::StageOutcome::Ran)) {
                // Everything inside the cascade stays in the formula space
                // Arjun was given; Vitri's simplify lift is applied once, below.
                let arjun_input = bundle
                    .arjun_input
                    .take()
                    .unwrap_or_else(|| compile_formula.clone());
                eprintln!(
                    "[mc] arjun: reduced in {}ms — {}v/{}c → {}v/{}c, lift=2^{}",
                    preprocessing.arjun_ms.unwrap_or(0) as u128,
                    arjun_input.num_vars(),
                    arjun_input.clauses().len(),
                    bundle.reduced.num_vars(),
                    bundle.reduced.clauses().len(),
                    bundle.count_lift.arjun_pow2,
                );
                // The input decides whether the retry is armed; the stage
                // report can only narrow it.
                let no_sbva_retry = (no_sbva_retry_armed
                    && matches!(bundle.stages.sbva, Some(vitri::bundle::StageOutcome::Ran)))
                .then_some(session);
                let ar = crate::frontend::record::arjun_reduction(bundle)
                    .expect("a kept plain-MC Arjun result carries its support");
                count_primary(run, &arjun_input, ar, primary_vtree, no_sbva_retry)
            } else {
                count_without_reduction(run, bundle.reduced, bundle.stages.arjun, primary_vtree)
            };
            let Some(Recovered { count, peak_pairs, answered_by, cascade_ms }) = recovered else {
                eprintln!("[mc] no answer: Shannon split gave up");
                return None;
            };
            Some(McOutcome {
                count: count << simplify_pow2,
                peak_pairs,
                cascade_ms: Some(cascade_ms),
                answered_by,
                reduction,
                preprocessing,
            })
        },
    );
    result.unwrap_or_else(|error| {
        eprintln!("[mc] preprocessing: Vitri's frontend failed: {error}");
        None
    })
}

/// No kept reduction: say why, then compile the simplified formula on the
/// vtree Vitri built for it, with the support-enum stage (on a fresh, bounded
/// Arjun pass) as its one fallback and Shannon split behind it. `None` when
/// nothing answered.
fn count_without_reduction(
    run: &Run<'_>,
    fallback_formula: CnfFormula,
    outcome: Option<vitri::bundle::StageOutcome>,
    vtree: crate::vtree::VtreeBuild,
) -> Option<Recovered> {
    // The single sink for every non-`Ran` disposition Vitri reports. The typed reason
    // is kept: a deterministic quality rejection and a budget give-up are
    // different findings.
    match outcome {
        Some(vitri::bundle::StageOutcome::Discarded(
            vitri::bundle::DiscardReason::NotSmaller,
        )) => eprintln!("[mc] arjun: the reduction grew the clauses — discarded"),
        Some(vitri::bundle::StageOutcome::Discarded(
            vitri::bundle::DiscardReason::NonInjectiveMap,
        )) => eprintln!(
            "[mc] arjun: the reduction has a non-injective variable map — discarded"
        ),
        Some(vitri::bundle::StageOutcome::Discarded(reason)) => {
            eprintln!("[mc] arjun: the reduction was discarded ({reason:?})")
        }
        Some(vitri::bundle::StageOutcome::GaveUp) => {
            eprintln!("[mc] arjun: gave up inside its budget")
        }
        Some(vitri::bundle::StageOutcome::Skipped(reason)) => {
            eprintln!("[mc] arjun: skipped ({reason:?})")
        }
        Some(vitri::bundle::StageOutcome::Ran) => unreachable!(
            "a Ran outcome must enter the reduced primary arm"
        ),
        Some(other) => {
            eprintln!("[mc] arjun: unavailable ({other:?})")
        }
        None => eprintln!("[mc] arjun: Vitri's frontend reported no Arjun stage"),
    }
    eprintln!("[mc] compile: no kept reduction — compiling the simplified formula on {}", run.config.vtree_spec());

    // A failure here enters the recovery like the primary compile's: this path
    // is taken whenever Arjun kept no reduction, often on the hard instances
    // that most need a fallback. Its one fallback is the support-enum stage,
    // which needs a support this count does not hold, so it buys one with a
    // second, tightly bounded exact-budget Arjun pass (`reduce_for_support`).
    let first = || count_attempt(run, attempt_from_vitri_build(fallback_formula.clone(), vtree, 0), run.deadline);
    let fallbacks = || {
        let ar = reduce_for_support(run.config, &fallback_formula, run.deadline)?;
        let count = support_enum_stage(
            &ar.formula,
            &ar.support,
            ar.lift_pow2,
            "fresh reduction",
            run.deadline,
        )?;
        Some(((count, 0), AnsweredBy::SupportEnumeration))
    };
    count_with_recovery(run, &fallback_formula, first, Some(&fallbacks), count_cofactor)
}

/// What became of Arjun's reduction, from the stage outcome Vitri reported:
/// kept, or run and not kept. `None` when Arjun did not run — it was skipped, or
/// the frontend reported no Arjun stage.
fn reduction(outcome: Option<&vitri::bundle::StageOutcome>) -> Option<Reduction> {
    match outcome {
        Some(vitri::bundle::StageOutcome::Ran) => Some(Reduction::Used),
        Some(vitri::bundle::StageOutcome::Skipped(_)) | None => None,
        Some(_) => Some(Reduction::Unavailable),
    }
}

/// The cascade's driver entry: count through [`count_mc`] and hand back
/// what it answered. The compile clock starts here, so `compile_ms` leaves out
/// the preprocessing and Arjun that [`count_mc`] runs first.
pub(crate) fn run_mc_count(run: &Run<'_>, formula: &CnfFormula) -> Result<McAnswer, crate::Error> {
    eprintln!("[mc] preprocessing: start");
    let clock = crate::budget::CompileClock::start(&run.config.vtree_time);
    // The stage that gave up has already said so on stderr, and there is no
    // other path behind this one.
    let out = count_mc(run, formula).ok_or_else(crate::Error::compile_gave_up)?;
    let telemetry = out.preprocessing;
    // Arjun has its own key, so preprocessing is the simplify chain alone.
    // The compile is what is left of the wall once all of the frontend's
    // preprocessing, Arjun included, and the vtree construction are out.
    let preprocess_ms = telemetry.simplify_ms.unwrap_or(0);
    let compile_ms = clock.compile_ms(telemetry.total_ms);
    Ok(McAnswer {
        count: out.count,
        peak_pairs: out.peak_pairs,
        preprocessing: PreprocessReport { preprocess_ms, ..PreprocessReport::of(&telemetry) },
        compile_ms,
        attribution: Attribution {
            answered_by: out.answered_by,
            reduction: out.reduction,
            arjun_ms: Some(telemetry.arjun_ms.unwrap_or(0) as u128),
            cascade_ms: out.cascade_ms,
        },
    })
}

/// Integer MC on an explicit `--vtree`: one Vitri simplify-only bundle, then
/// the streaming per-component compile of the reduced formula, recovered by
/// Shannon split when it fails. There is no fallback: this is already the
/// single-construction route.
///
/// The reduced formula is already in the exact variable space to compile; the
/// bundle's lift is the complete lift from that space to the input and is
/// applied once, after recovery returns its count.
pub(crate) fn run_mc_streaming(run: &Run<'_>, formula: &CnfFormula) -> Result<McAnswer, crate::Error> {
    let bundle = crate::frontend::mc_streaming_preprocess(formula, run.config, run.deadline)
        .map_err(crate::Error::stage_failed("mc preprocessing"))?;
    let vitri::bundle::PreprocessBundle {
        reduced,
        record,
        stages,
        count_lift,
        telemetry,
        ..
    } = bundle;
    eprintln!(
        "[mc] preprocessing: simplify={:?}, arjun={:?}, {}v/{}c, lift=2^{}, {}ms",
        stages.simplify,
        stages.arjun,
        reduced.num_vars(),
        reduced.clauses().len(),
        count_lift.total_pow2(),
        telemetry.total_ms,
    );
    if telemetry.backbone_found == formula.num_vars() as usize && formula.num_vars() > 0 {
        // Vitri keeps one forced variable live so downstream compilers do
        // not need a zero-variable special case; the backbone count says the
        // whole formula was forced.
        eprintln!("[mc] preprocessing: every variable is a backbone");
    }

    let lift_pow2 = count_lift.total_pow2() as usize;
    let preprocessing = PreprocessReport::of(&telemetry);
    // Preprocessing that settles the count leaves nothing to compile: no peak,
    // no compile time and no cascade.
    if record.unsat || reduced.num_vars() == 0 {
        let count = if record.unsat {
            eprintln!("[mc] preprocessing: refuted the formula");
            num_bigint::BigUint::from(0u32)
        } else {
            eprintln!("[mc] preprocessing: resolved every variable — the count is 2^{lift_pow2}");
            num_bigint::BigUint::from(1u32) << lift_pow2
        };
        return Ok(McAnswer {
            count,
            peak_pairs: 0,
            preprocessing,
            compile_ms: 0,
            attribution: Attribution {
                answered_by: AnsweredBy::Preprocessing,
                reduction: None,
                arjun_ms: None,
                cascade_ms: None,
            },
        });
    }
    eprintln!("[vtree] {}", run.config.vtree_spec());
    eprintln!("[mc] compile: start");
    // In-loop sub-vtree builds bill to `vtree_ms`, not `compile_ms`.
    let clock = crate::budget::CompileClock::start(&run.config.vtree_time);
    // The first compile and each cofactor's stream over their own components,
    // building each vtree as the compile reaches it.
    let first = || super::count_streaming(run, &reduced, None);
    let cofactor = |run: &Run<'_>, form: &CnfFormula| super::count_streaming(run, form, None);
    let Recovered { count, peak_pairs, answered_by, cascade_ms } =
        count_with_recovery(run, &reduced, first, None, cofactor)
            .ok_or_else(crate::Error::compile_gave_up)?;
    Ok(McAnswer {
        count: count << lift_pow2,
        peak_pairs,
        preprocessing,
        compile_ms: clock.compile_ms(0),
        attribution: Attribution {
            answered_by,
            reduction: None,
            arjun_ms: None,
            cascade_ms: Some(cascade_ms),
        },
    })
}
