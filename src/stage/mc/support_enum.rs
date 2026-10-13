//! The model count's support-enumeration fallback.
//!
//! Entered among the fallbacks once the first compile has failed (see
//! `recovery.rs`), it counts the Arjun-reduced formula by
//! enumerating the feasible assignments to Arjun's independent support
//! ([`crate::frontend::record::ArjunReduction::support`]) through the
//! shared [`show_enum_fastpath`](crate::enumeration::show_enum_fastpath). Each
//! feasible support assignment extends to exactly one model, so the
//! enumeration is exact; the estimate below only decides whether to keep
//! going, and a count is emitted only when the enumeration runs out.
//!
//! [`plan_support_enum`] checks the preconditions and carves the window out of
//! what [`shannon_split_reserve_ms`] leaves the recovery cascade. Then three phases:
//! **estimate** the feasible-projection count N by random paths
//! ([`estimate_projected_count`]); **probe** for [`PROBE_WALL_MS`], uncommitted;
//! **commit** the rest of the window, resuming the probe's solver, when and only
//! when N̂ ≤ [`COMMIT_MAX_MODELS`]. [`commit_decision`] is asked on the one
//! estimate before the probe and again at its wall. The estimate only gates; the
//! reserve rule sets the budget, and [`TALLY_CEILING`] bounds a committed run.
//!
//! The design and its cost bounds: `docs/support-enum-stage.md`.

use std::time::Instant;

use vitri::cnf::CnfFormula;

use crate::enumeration::{EnumLimits, PathEstimate, show_enum_fastpath};
use crate::problem::ShowSet;

use super::windows::{shannon_split_reserve_ms, support_enum_arjun_retry_ms};

/// Largest independent support the stage will enumerate over: the shared
/// fastpath's band ceiling, past which it declines anyway.
const MAX_SUPPORT: usize = crate::enumeration::MAX_ENUM_SHOW;

/// Model cap handed to the projected-AllSAT enumerator. Larger than the
/// projected count's shipped 200k default because this stage runs only after a
/// compile failure: there is no compile path left to starve, so the only cost of
/// a higher cap is enumeration wall the cascade would otherwise spend
/// Shannon-splitting a formula that already blew up. Either cap (models or wall)
/// simply yields `None` and falls through — never a wrong answer.
const MODEL_CAP: u64 = 2_000_000;

/// Below this much enumeration wall the stage declines: a projected-AllSAT
/// enumeration over a several-hundred-variable support runs into six-figure model
/// counts and needs a long window, so a short one only burns wall the recovery
/// cascade is owed.
const MIN_ENUM_WALL_MS: u64 = 600_000;

/// Uncommitted first slice. Long enough that an enumeration small enough to
/// finish at all finishes inside it, and short enough that a candidate that busts
/// has spent a bounded, affordable part of the stage window by the time the gate
/// is asked again.
const PROBE_WALL_MS: u64 = 180_000;

/// Commit ceiling on the estimated feasible-projection count. An enumeration
/// whose feasible set is at most a million projections finishes inside the window
/// the reserve rule leaves; past that it does not, so an N̂ above this is a
/// refusal.
const COMMIT_MAX_MODELS: f64 = 1_000_000.0;

/// Random paths walked by the estimator, and the wall on each. One path costs one
/// assumption-solve per support var, so 8 × 10 s bounds the whole estimate at ~80 s
/// of the enumeration window in the worst case and typically costs a few seconds.
const ESTIMATOR_SAMPLES: u32 = 8;
const ESTIMATOR_SAMPLE_WALL_MS: u64 = 10_000;

/// Give up estimating after this many cut paths in total. An instance whose
/// single paths already outrun the 10 s path wall is one the enumeration cannot
/// finish either, so there is no point paying for the remaining samples.
const ESTIMATOR_MAX_TRUNCATED: u32 = 2;

/// Tally past which a committed enumeration stops, no matter what the estimator
/// said. Admission means the count is at most [`COMMIT_MAX_MODELS`], so a tally
/// beyond that disproves the admission; the 5% headroom lets a count sitting
/// exactly at the gate finish. No bound is derived from N̂: the estimator
/// under-reads the count a committed run reaches by up to two orders of
/// magnitude, and the error does not track the run's size.
const TALLY_CEILING: u64 = (COMMIT_MAX_MODELS * 1.05) as u64;
const _: () = assert!(TALLY_CEILING as f64 >= COMMIT_MAX_MODELS);

/// Decide whether to run the support-enum stage: the stage's whole wall
/// (estimate, probe and commit together), or the reason to skip it for the log.
///
/// `remaining_ms` is the wall left before the program deadline (`None` = no
/// `--time-limit-ms` at all, in which case there is no budget to carve and the
/// stage declines rather than enumerate unbounded).
fn plan_support_enum(
    support_len: usize,
    remaining_ms: Option<u64>,
) -> Result<u64, &'static str> {
    if support_len == 0 {
        return Err("Arjun reported no independent support");
    }
    if support_len > MAX_SUPPORT {
        return Err("independent support larger than the enumeration band");
    }
    let Some(remaining_ms) = remaining_ms else {
        return Err("no time limit to budget against");
    };
    let enum_wall_ms = remaining_ms.saturating_sub(shannon_split_reserve_ms(remaining_ms));
    if enum_wall_ms < MIN_ENUM_WALL_MS {
        return Err("too little wall left for a worthwhile enumeration");
    }
    Ok(enum_wall_ms)
}

/// Estimate the feasible-projection count N by walking random paths down the
/// projection tree (the engine's `estimate_count_by_random_paths`), one path at
/// a time until [`ESTIMATOR_SAMPLES`] are drawn or [`ESTIMATOR_MAX_TRUNCATED`]
/// are cut.
///
/// The stage's whole question is "will this enumeration finish inside the wall
/// left?", i.e. is N small. The estimator therefore has to be trustworthy in the
/// direction that costs wall: it must not report "small" for a huge N. That is
/// why it walks paths rather than random restrictions, which collapse to 0 on
/// supports full of near-forced variables (see `estimate_count_by_random_paths`).
///
/// Everything is in raw feasible-projection units (the engine restricts itself to
/// the constrained-show vars), which is the same unit as the model cap and the
/// rate the probe reports.
fn estimate_projected_count(
    formula: &CnfFormula,
    show_vec: &[tididi::vtree::VarId],
) -> PathEstimate {
    let mut acc = PathEstimate::default();
    // Drawn one path at a time so the truncation cut-off costs at most one extra
    // path: an instance whose paths already outrun the path wall gets out after
    // ~2 samples instead of paying for all 8.
    let drawn =
        |a: &PathEstimate| a.weights.len() as u32 + a.truncated;
    while drawn(&acc) < ESTIMATOR_SAMPLES && acc.truncated < ESTIMATOR_MAX_TRUNCATED {
        let est = crate::enumeration::estimate_count_by_random_paths(
            formula,
            show_vec,
            1,
            std::time::Duration::from_millis(ESTIMATOR_SAMPLE_WALL_MS),
            path_seed(drawn(&acc)),
        );
        if est.unsat {
            acc.unsat = true;
            break;
        }
        acc.truncated += est.truncated;
        acc.weights.extend(est.weights);
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

/// N̂ for the log: the mean of the completed paths, or "unmeasured".
fn n_hat_text(est: &PathEstimate) -> String {
    match est.mean() {
        Some(n) => format!("{n:.3e}"),
        None => "unmeasured".to_string(),
    }
}

/// Decide whether to enumerate, and hand back the budget: the whole of
/// `available_ms`, the stage wall still left net of the cascade's reserve.
///
/// [`run_support_enum`] asks it twice on one estimate: before the probe with
/// the stage's whole wall and nothing tallied, and at the probe's wall with what
/// is left and what was counted. The two probe-dependent tests
/// (`available_ms == 0` and the tally ceiling) come first and last and can only
/// refuse, so a refusal at entry is one the exit ask would also have made.
///
/// With no completed path there is no estimate, and the stage bails: unmeasured
/// is not small.
fn commit_decision(
    est: &PathEstimate,
    available_ms: u64,
    tallied: u64,
) -> Result<u64, &'static str> {
    if available_ms == 0 {
        return Err("no stage wall left to commit");
    }
    if est.unsat {
        // Nothing to enumerate at all: two solves finish it.
        return Ok(available_ms);
    }
    let Some(n_hat) = est.mean() else {
        return Err("no sampled path completed — the count could not be estimated");
    };
    if n_hat.is_nan() || n_hat > COMMIT_MAX_MODELS {
        return Err("estimated projected count above the commit threshold");
    }
    if tallied >= TALLY_CEILING {
        return Err("the probe already tallied past the count the gate admits");
    }
    Ok(available_ms)
}

/// Run the enumeration: estimate, probe, commit. Returns the model count of
/// `reduced`, in its own space, or `None` when the stage bailed.
///
/// Everything policy-shaped (band, the 2^K-sweep-then-AllSAT order) is the
/// shared [`show_enum_fastpath`](crate::enumeration::show_enum_fastpath)'s; this only
/// supplies the unweighted tally, the two call-site budget parameters, and the
/// probe/commit control the enumerator takes as a caller callback. The
/// projected count's near-full feasibility skip is not set here: it protects a
/// compile path that still has wall left, and by this point compile has
/// already failed.
fn run_support_enum(
    reduced: &CnfFormula,
    support: &ShowSet,
    probe_wall_ms: u64,
    enum_wall_ms: u64,
) -> Option<num_bigint::BigUint> {
    use crate::enumeration::{AllsatStop, AllsatCtl, Models, WallGrant};
    use tididi::vtree::VarId;
    use std::time::Duration;

    // The estimate gates entry. Taken once, here, and read again at the
    // probe's wall: the estimator sees only the formula and the show set, so a
    // second draw could only add sampling noise. The show vector comes from
    // the converter the shared fastpath uses.
    let show_vec: Vec<VarId> = crate::enumeration::show_vars(support);
    let t_est = Instant::now();
    let est = estimate_projected_count(reduced, &show_vec);
    eprintln!(
        "[mc] support-enumeration: estimate in {}ms: N̂={} ({} paths, max {:.3e}{}{})",
        t_est.elapsed().as_millis(),
        n_hat_text(&est),
        est.weights.len(),
        est.weights.iter().copied().fold(0.0f64, f64::max),
        if est.truncated > 0 {
            format!(", {} cut", est.truncated)
        } else {
            String::new()
        },
        if est.unsat { ", formula UNSAT" } else { "" },
    );
    // Nothing tallied and the whole stage wall still ahead, so the only rules
    // that can refuse here are the estimate's own — see [`commit_decision`].
    if let Err(why) = commit_decision(&est, enum_wall_ms, 0) {
        eprintln!("[mc] support-enumeration: skipped (estimate): {why}");
        return None;
    }

    let stage_deadline = Instant::now() + Duration::from_millis(enum_wall_ms);
    // Fires once, when the probe's wall runs out: weigh the entry estimate
    // against what the probe actually did and either commit (resuming this
    // same solver and its blocking-clause DB) or bail. A second call means the
    // committed window itself ran out — that one is a real bust.
    let mut committed = false;
    let mut at_wall = |models: u64, elapsed: Duration| -> Option<WallGrant> {
        if committed {
            return None;
        }
        committed = true;
        let rho = models as f64 / elapsed.as_secs_f64().max(1e-3);
        let available_ms = crate::budget::left_ms(Some(stage_deadline)).unwrap_or(0);
        // The same estimate the entry gate admitted on — the probe cannot
        // inform it, so re-drawing it would only add sampling noise to a
        // decision already taken on this number.
        let verdict = commit_decision(&est, available_ms, models);
        eprintln!(
            "[mc] support-enumeration: probe: {models} models in {:.1}s ({rho:.0}/s) — \
             N̂={} from the entry estimate, {}s of stage wall left ⇒ {}",
            elapsed.as_secs_f64(),
            n_hat_text(&est),
            available_ms / 1000,
            match &verdict {
                Ok(ms) => {
                    format!("COMMIT {}s (abort past {TALLY_CEILING} models)", ms / 1000)
                }
                Err(why) => format!("BAIL ({why})"),
            },
        );
        // Every commit rests on the same admission (`N̂ ≤ COMMIT_MAX_MODELS`)
        // and so carries the same bound: the gate's own ceiling, attached to
        // the grant (see `WallGrant`).
        verdict.ok().map(|ms| WallGrant {
            extra: Duration::from_millis(ms),
            abort_at_models: TALLY_CEILING,
        })
    };
    // The callback also connects the wall terminator, because a bust must
    // never overrun into the cascade's reserve and a check made only between
    // solves does not bound a solve that is already running.
    let mut ctl = AllsatCtl {
        at_wall: Some(&mut at_wall),
        ..Default::default()
    };
    let out = show_enum_fastpath(
        reduced,
        support,
        Models::default(),
        EnumLimits {
            allsat_wall_ms: probe_wall_ms,
            model_budget_override: Some(MODEL_CAP),
            skip_near_full: false,
            deadline: None,
        },
        &mut ctl,
    );
    match out {
        Some((count, method, elapsed_ms)) => {
            eprintln!(
                "[mc] support-enumeration: {method} counted the Arjun-reduced formula in {elapsed_ms}ms"
            );
            Some(count)
        }
        None => {
            eprintln!(
                "[mc] support-enumeration: enumeration stopped: {} ({} models tallied)",
                match ctl.bust {
                    Some(AllsatStop::Wall) => "wall budget spent between solves".to_string(),
                    Some(AllsatStop::Terminated) => "wall budget cut a solve in flight".to_string(),
                    Some(AllsatStop::ModelCap) => "model cap".to_string(),
                    Some(AllsatStop::TallyCeiling) => format!(
                        "gate ceiling exceeded (tally > {TALLY_CEILING}, the count the \
                         entry gate admits)"
                    ),
                    None => "declined before enumerating".to_string(),
                },
                ctl.models,
            );
            None
        }
    }
}

/// The stage body, shared by both fallback sites (behind the primary compile
/// and behind the count without a reduction): plan, log, enumerate, and fold
/// the reduction multiplier. Returns a count in the var space of the formula
/// `reduced` was derived from — exactly what a fallback must return.
pub(crate) fn support_enum_stage(
    reduced: &CnfFormula,
    support: &ShowSet,
    lift_pow2: u32,
    // Where the reduction came from, for the log ("kept reduction" / "fresh
    // reduction").
    site: &'static str,
    program_deadline: Option<Instant>,
) -> Option<num_bigint::BigUint> {
    let remaining_ms = crate::budget::left_ms(program_deadline);
    match plan_support_enum(support.len(), remaining_ms) {
        Err(why) => {
            eprintln!("[mc] support-enumeration: skipped ({site}): {why}");
            None
        }
        Ok(enum_wall_ms) => {
            eprintln!(
                "[mc] support-enumeration: the compile failed ({site}) — enumerating {} support vars over the \
                 Arjun-reduced formula ({}v/{}c): probe {}s, stage ceiling {}s (model cap {}), \
                 {}s of wall left",
                support.len(),
                reduced.num_vars(),
                reduced.clauses().len(),
                PROBE_WALL_MS / 1000,
                enum_wall_ms / 1000,
                MODEL_CAP,
                remaining_ms.unwrap_or(0) / 1000,
            );
            let t_enum = Instant::now();
            match run_support_enum(reduced, support, PROBE_WALL_MS, enum_wall_ms) {
                Some(reduced_count) => {
                    eprintln!(
                        "[mc] support-enumeration: counted in {}ms — lifting the reduced count by 2^{}",
                        t_enum.elapsed().as_millis(),
                        lift_pow2,
                    );
                    Some(reduced_count << lift_pow2 as usize)
                }
                None => {
                    eprintln!(
                        "[mc] support-enumeration: stopped after {}ms — the next fallback runs",
                        t_enum.elapsed().as_millis(),
                    );
                    None
                }
            }
        }
    }
}

/// Obtain an independent support for the count without a reduction by
/// re-running the anytime reducer under [`support_enum_arjun_retry_ms`].
///
/// Why a re-run: that count is reached whenever Arjun kept no reduction
/// (killed at its deadline, gave up, skipped, or discarded as not smaller or
/// non-injective), so it has no support to hand the stage. A bounded slice of
/// the recovery window buys one, as the cascade's per-cofactor re-Arjun already
/// does.
pub(super) fn reduce_for_support(
    parent: &crate::config::Config,
    form: &CnfFormula,
    program_deadline: Option<Instant>,
) -> Option<crate::frontend::record::ArjunReduction> {
    let remaining_ms = crate::budget::left_ms(program_deadline);
    let Some(budget_ms) = support_enum_arjun_retry_ms(remaining_ms) else {
        eprintln!(
            "[mc] support-enumeration: skipped (no kept reduction): {}s left is too little \
             to re-run Arjun for a support",
            remaining_ms.unwrap_or(0) / 1000,
        );
        return None;
    };
    eprintln!(
        "[mc] support-enumeration: no kept reduction — re-running Arjun for an \
         independent support (budget {}ms) on {}v/{}c",
        budget_ms,
        form.num_vars(),
        form.clauses().len(),
    );
    let t = Instant::now();
    // Vitri's Arjun-only operation on the exact allowance chosen above.
    // Simplification is deliberately disabled: this retry asks only for the
    // independent support that the first attempt could not carry across its
    // process boundary.
    let config = crate::frontend::request::arjun_stage(
        parent,
        vitri::cnf::Mode::Mc,
        None,
        std::time::Duration::from_millis(budget_ms),
        vitri::config::ArjunClauseGrowth::KeepSound,
    );
    let reduced = vitri::bundle::preprocess(form, &vitri::cnf::CnfMeta::default(), &config)
        .ok()
        .and_then(crate::frontend::record::arjun_reduction);
    match reduced {
        Some(ar) => {
            eprintln!(
                "[mc] support-enumeration: arjun reduced in {}ms — {}v/{}c, |support|={}, lift=2^{}",
                t.elapsed().as_millis(),
                ar.formula.num_vars(),
                ar.formula.clauses().len(),
                ar.support.len(),
                ar.lift_pow2,
            );
            Some(ar)
        }
        None => {
            eprintln!(
                "[mc] support-enumeration: arjun produced nothing in {}ms — Shannon split \
                 next",
                t.elapsed().as_millis(),
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/support_enum.rs"]
mod tests;
