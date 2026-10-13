//! What each fallback behind the primary compile is allowed to spend.
//!
//! Every window here is pure arithmetic over the declared time limit, the
//! program deadline and the clock, so the whole window order is unit-testable
//! without a run: the unreduced retry's reserve, the predicate-gated no-SBVA
//! retry and the support-enumeration re-reduce. Keeping them together keeps
//! two stages from carving the same wall twice.

use super::support_enum::shannon_split_reserve_ms;

/// Wall floor below which the count without a reduction will not spend any of
/// the recovery window looking for a support. Above it the retry is
/// affordable; below it Shannon split keeps the whole window.
const SUPPORT_MIN_REMAINING_MS: u64 = 1_800_000;

/// Ceiling on that retry, whatever the wall.
const SUPPORT_ARJUN_MAX_MS: u64 = 600_000;

/// Below this the no-SBVA retry is not entered at all. The stage has to run a
/// whole Arjun reduce (its own floor is 5 s), build a vtree and compile; under
/// half a minute it can only take wall away from the unreduced retry and
/// Shannon split that follow it.
const NO_SBVA_RETRY_MIN_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

/// Budget for re-running Arjun when the count runs without a reduction, or
/// `None` when it must not try at all. Pure, so the arithmetic is
/// unit-testable.
///
/// Carves against the same reserve rule the stage itself uses — the one
/// [`shannon_split_reserve_ms`] defines, not a second copy of it — and then takes at
/// most a third of what is left, capped at [`SUPPORT_ARJUN_MAX_MS`], so a retry
/// can never leave the stage behind it too little wall to clear its own entry bar.
pub(super) fn support_enum_arjun_retry_ms(remaining_ms: Option<u64>) -> Option<u64> {
    let remaining = remaining_ms?;
    if remaining < SUPPORT_MIN_REMAINING_MS {
        return None;
    }
    let available = remaining.saturating_sub(shannon_split_reserve_ms(remaining));
    Some(SUPPORT_ARJUN_MAX_MS.min(available / 3))
}

/// Wall reserved at the end of the run for the unreduced retry's from-scratch
/// compile of the unreduced formula. One source for both places that reserve
/// it: the primary deadline in `count_primary` (which caps the primary compile
/// at `program_deadline − UNREDUCED_RETRY_RESERVE`) and
/// [`unreduced_retry_window`]'s no-wall-limit case. Two literals here would be
/// two numbers to keep in sync.
pub(super) const UNREDUCED_RETRY_RESERVE: std::time::Duration = std::time::Duration::from_millis(250_000);

/// Reserve window for the unreduced retry — the third fallback — which runs
/// on any failure of the primary compile. Pure so it can be
/// unit-tested.
///
/// `primary_deadline` is `Some` only when the absolute [`UNREDUCED_RETRY_RESERVE`]
/// armed (long window). The window is then what is left when the stage is
/// entered, less what Shannon split behind it is owed, and never less than
/// the tail the primary deadline already reserved — see the branch below for
/// why. `None` is the short-window regime (<=500s remaining, a two-minute
/// limit included): the primary compile and its recovery would otherwise
/// consume the whole wall, leaving the unreduced retry nothing, so a quarter
/// of the remaining wall goes to it before Shannon split. Degenerate sub-2s
/// tails skip (return `None`). With no wall limit at all, reserve is the
/// [`UNREDUCED_RETRY_RESERVE`] constant.
pub(super) fn unreduced_retry_window(
    primary_deadline: Option<std::time::Instant>,
    program_deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> Option<std::time::Duration> {
    use std::time::Duration;
    match (primary_deadline, program_deadline) {
        // Armed (long window). `pd - adl` is what the primary deadline reserved
        // for this stage, and it is what the stage actually has only when the
        // primary compile runs all the way to `adl`. It usually does not: the
        // primary compile is cut well short of `adl` by its memory and pair
        // limits, and tail arithmetic keyed to the reserved figure would hand
        // the wall the primary compile left over to Shannon split behind it
        // instead.
        //
        // The window also sizes the retry's vtree construction (a third of
        // it, `budget::vtree_deadline`), so a window cut to the reserved
        // figure buys a worse tree as well as less time to compile on it.
        //
        // So the reserved tail is a floor on this stage's window, not a cap on
        // it: take what is left, less what Shannon split behind this stage is
        // owed, and never less than the tail the primary deadline reserved.
        // `shannon_split_reserve_ms` is the one definition of that debt in the tree —
        // the support-enum stage ahead of this one carves against it too, and a
        // second rule here would be a second answer to the same question.
        //
        // Not clamped to what remains: the caller already clamps the window it
        // builds from this at `program_deadline`, and clamping here as well would
        // be the same rule written twice.
        (Some(adl), Some(pd)) => {
            let left = pd.saturating_duration_since(now);
            let owed_to_cascade =
                Duration::from_millis(shannon_split_reserve_ms(left.as_millis() as u64));
            Some(
                left.saturating_sub(owed_to_cascade)
                    .max(pd.saturating_duration_since(adl)),
            )
        }
        // Short window: a quarter of what remains, skipped when that is under
        // 2 s (under 8 s left).
        (None, Some(pd)) => {
            let left = pd.saturating_duration_since(now);
            if left / 4 < Duration::from_secs(2) {
                return None;
            }
            Some(left / 4)
        }
        // No wall limit at all: the reserved-tail constant, armed or not.
        (_, None) => Some(UNREDUCED_RETRY_RESERVE),
    }
}

/// Share of the total declared wall (`--time-limit-ms`) reserved for the
/// predicate-gated no-SBVA retry (the first fallback).
///
/// A quarter covers the re-reduce and the recompile, and leaves the unreduced
/// retry and Shannon split the rest. Deliberately a fraction of the declared wall, not of what happens to remain:
/// the window a stage is worth is a property of the run's budget, and keying it
/// to "remaining" would make an earlier-OOMing instance get a *larger* retry
/// than a later-OOMing one.
const NO_SBVA_RETRY_RESERVE_FRAC: f64 = 0.25;

/// Vitri construction requested for the coloring-like no-SBVA retry.
///
/// This is caller policy, not a vtree path of this crate's: Vitri parses the
/// spec and owns the construction. The retry is entered only for coloring-like inputs,
/// where the hypergraph construction keeps the clause structure that motivated
/// removing SBVA in the first place.
pub(super) const NO_SBVA_RETRY_VTREE_SPEC: &str = "hypergraph-bisect:imbalance=0.40";

/// [`NO_SBVA_RETRY_RESERVE_FRAC`] of the declared wall — the one derivation of the
/// stage's "want", shared by [`no_sbva_retry_reserve`] (which caps it at what
/// remains) and the primary deadline in `count_primary` (which carves it out
/// of the primary compile's window when the retry is armed, so a primary
/// compile that runs to its deadline still leaves the stage its reserve).
pub(super) fn no_sbva_retry_window(total_ms: u64) -> std::time::Duration {
    std::time::Duration::from_millis((total_ms as f64 * NO_SBVA_RETRY_RESERVE_FRAC) as u64)
}

/// Window for the predicate-gated no-SBVA retry: [`no_sbva_retry_window`], never more
/// than what is actually left, `None` when that is under
/// [`NO_SBVA_RETRY_MIN_WINDOW`] (skip the stage). Pure, so it is unit-tested
/// like [`unreduced_retry_window`], whose shape it deliberately mirrors — this is
/// one more step of the same window order, not a second budget mechanism.
///
/// With no declared wall at all the stage gets [`UNREDUCED_RETRY_RESERVE`], the same
/// fixed window the unreduced retry takes in that regime: an unbounded run
/// still must not let one stage eat the whole cascade.
pub(super) fn no_sbva_retry_reserve(
    time_limit_ms: Option<u64>,
    program_deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> Option<std::time::Duration> {
    match (time_limit_ms, program_deadline) {
        (Some(total_ms), Some(pd)) => {
            let window = no_sbva_retry_window(total_ms).min(pd.saturating_duration_since(now));
            (window >= NO_SBVA_RETRY_MIN_WINDOW).then_some(window)
        }
        // No declared wall (the deadline is derived from it, so the two are
        // absent together): the fixed reserved tail.
        _ => Some(UNREDUCED_RETRY_RESERVE),
    }
}

#[cfg(test)]
#[path = "tests/windows.rs"]
mod tests;
