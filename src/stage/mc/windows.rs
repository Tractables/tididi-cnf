//! What each fallback behind the primary compile is allowed to spend, and how
//! long one step of the primary compile may run before the stall recompile
//! takes over.
//!
//! Every window here is pure arithmetic over the declared time limit, the
//! program deadline and the clock, so the whole window order is unit-testable
//! without a run: the primary compile's stall rule, the wall Shannon split is
//! owed, the unreduced retry's reserve, the predicate-gated no-SBVA retry and
//! the support-enumeration re-reduce. Keeping them together keeps two stages
//! from carving the same wall twice.

use crate::compile::StallRule;

/// The stall rule's per-step wall as a share of the declared time limit: one
/// internal vtree-node step of the primary compile may run a sixth of the
/// run's limit, 20 s at a two-minute limit. Chosen together with
/// [`STALL_MIN_STEPS_LEFT`].
const STALL_WALL_DIVISOR: u64 = 6;

/// The fewest internal steps, the step itself included, that must be left in
/// its component's bottom-up schedule for the stall rule to bound a step
/// ([`StallRule::min_left`]). A step that stalls this far from the root has
/// most of the compile still ahead of it; one that stalls near the root more
/// often belongs to a compile that finishes, and is left to run.
const STALL_MIN_STEPS_LEFT: usize = 50;

/// The stall rule the model count's primary compile runs under: a wall of
/// [`STALL_WALL_DIVISOR`]'s share of the declared limit on each internal step
/// with at least [`STALL_MIN_STEPS_LEFT`] steps left. `None` without a time
/// limit, where no step has a wall.
pub(super) fn stall_rule(time_limit_ms: Option<u64>) -> Option<StallRule> {
    let total_ms = time_limit_ms?;
    Some(StallRule {
        wall: std::time::Duration::from_millis(total_ms / STALL_WALL_DIVISOR),
        min_left: STALL_MIN_STEPS_LEFT,
    })
}

/// Absolute floor on the wall reserved for the recovery cascade behind a
/// fallback.
const MIN_RESERVE_MS: u64 = 300_000;

/// The reserve rule: the wall a fallback always leaves Shannon split behind
/// it, out of `remaining_ms`: a quarter, and never less than
/// [`MIN_RESERVE_MS`]. The support-enumeration stage, the support hunt ahead
/// of it and the unreduced retry all carve against it.
pub(super) fn shannon_split_reserve_ms(remaining_ms: u64) -> u64 {
    MIN_RESERVE_MS.max(remaining_ms / 4)
}

/// Wall floor below which the count without a reduction will not spend any of
/// the recovery window looking for a support. Above it the retry is
/// affordable; below it Shannon split keeps the whole window.
const SUPPORT_MIN_REMAINING_MS: u64 = 1_800_000;

/// Ceiling on that retry, whatever the wall.
const SUPPORT_ARJUN_MAX_MS: u64 = 600_000;

/// Below this a fallback that reduces again, the no-SBVA retry or the
/// monotone reduction, is not entered at all. The stage has to run a whole
/// Arjun reduce (its own floor is 5 s), build a vtree and compile; under half
/// a minute it can only take wall away from the stages that follow it.
const REDUCE_RETRY_MIN_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

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
/// compile of the unreduced formula: the primary deadline in `count_primary`
/// caps the primary compile at `program_deadline − UNREDUCED_RETRY_RESERVE`,
/// and [`unreduced_retry_window`] takes it when the run has no wall.
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
        // Armed (long window). `program - primary` is what the primary
        // deadline reserved for this stage, and it is what the stage actually
        // has only when the primary compile runs all the way to `primary`. It
        // usually does not: the primary compile is cut well short of it by its
        // memory and pair limits, and tail arithmetic keyed to the reserved
        // figure would hand the wall the primary compile left over to Shannon
        // split behind it instead.
        //
        // The window also sizes the retry's vtree construction (a third of
        // it, `budget::vtree_deadline`), so a window cut to the reserved
        // figure buys a worse tree as well as less time to compile on it.
        //
        // So the reserved tail is a floor on this stage's window, not a cap on
        // it: take what is left, less what Shannon split behind this stage is
        // owed ([`shannon_split_reserve_ms`]), and never less than the tail the
        // primary deadline reserved. The caller clamps the window at
        // `program_deadline`.
        (Some(primary), Some(program)) => {
            let left = program.saturating_duration_since(now);
            let owed_to_cascade =
                Duration::from_millis(shannon_split_reserve_ms(left.as_millis() as u64));
            Some(
                left.saturating_sub(owed_to_cascade)
                    .max(program.saturating_duration_since(primary)),
            )
        }
        // Short window: a quarter of what remains, skipped when that is under
        // 2 s (under 8 s left).
        (None, Some(program)) => {
            let left = program.saturating_duration_since(now);
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
/// retry and Shannon split the rest. A fraction of the declared wall, not of
/// what happens to remain: the window a stage is worth is a property of the
/// run's budget, and keying it to "remaining" would give an instance whose
/// primary compile fails earlier a larger retry than one whose compile fails
/// later.
const NO_SBVA_RETRY_RESERVE_FRAC: f64 = 0.25;

/// What the no-SBVA retry wants: [`NO_SBVA_RETRY_RESERVE_FRAC`] of the
/// declared wall. [`no_sbva_retry_window`] caps it at what remains, and the
/// primary deadline in `count_primary` carves it out of the primary compile's
/// window when the retry is armed, so a primary compile that runs to its
/// deadline still leaves the stage its share.
pub(super) fn no_sbva_retry_want(total_ms: u64) -> std::time::Duration {
    std::time::Duration::from_millis((total_ms as f64 * NO_SBVA_RETRY_RESERVE_FRAC) as u64)
}

/// Window for the predicate-gated no-SBVA retry: [`no_sbva_retry_want`], never
/// more than what is actually left, `None` when that is under
/// [`REDUCE_RETRY_MIN_WINDOW`] (skip the stage). The same shape as
/// [`unreduced_retry_window`]: one more step of the same window order.
///
/// With no declared wall at all the stage gets [`UNREDUCED_RETRY_RESERVE`], the same
/// fixed window the unreduced retry takes in that regime: an unbounded run
/// still must not let one stage eat the whole cascade.
pub(super) fn no_sbva_retry_window(
    time_limit_ms: Option<u64>,
    program_deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> Option<std::time::Duration> {
    match (time_limit_ms, program_deadline) {
        (Some(total_ms), Some(program)) => {
            let window = no_sbva_retry_want(total_ms).min(program.saturating_duration_since(now));
            (window >= REDUCE_RETRY_MIN_WINDOW).then_some(window)
        }
        // No declared wall (the deadline is derived from it, so the two are
        // absent together): the fixed reserved tail.
        _ => Some(UNREDUCED_RETRY_RESERVE),
    }
}

/// Window for the monotone reduction, the fallback of the count without a
/// reduction when Arjun skipped the formula as monotone: what is left, less
/// what Shannon split behind it is owed ([`shannon_split_reserve_ms`]), `None`
/// when that is under [`REDUCE_RETRY_MIN_WINDOW`] (skip the stage). At a
/// two-minute limit the reserve is all that is left, so the stage runs only at
/// longer limits. With no wall at all, the fixed [`UNREDUCED_RETRY_RESERVE`],
/// as for the no-SBVA retry.
pub(super) fn monotone_reduction_window(
    program_deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> Option<std::time::Duration> {
    let Some(program) = program_deadline else {
        return Some(UNREDUCED_RETRY_RESERVE);
    };
    let left = program.saturating_duration_since(now);
    let owed = std::time::Duration::from_millis(shannon_split_reserve_ms(left.as_millis() as u64));
    let window = left.saturating_sub(owed);
    (window >= REDUCE_RETRY_MIN_WINDOW).then_some(window)
}

#[cfg(test)]
#[path = "tests/windows.rs"]
mod tests;
