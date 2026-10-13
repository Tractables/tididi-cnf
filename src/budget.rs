//! Caller-side wall slicing: how the driver carves its per-CNF wall into the
//! sub-budgets it hands down.
//!
//! These are caller policies: TiDiDi knows the run wall and the downstream
//! stages that need time. Vitri receives the resulting typed deadline and owns
//! construction-internal budget division.

/// `ratio` of the run's time limit clamped to `[min, max]`, or `abs_default`
/// when the run has none.
pub(crate) fn resolve_scaled(
    time_limit_ms: Option<u64>,
    ratio: f64,
    abs_default: u64,
    min: u64,
    max: u64,
) -> u64 {
    time_limit_ms.map_or(abs_default, |wall| ((wall as f64 * ratio) as u64).clamp(min, max))
}

/// Milliseconds left before `deadline`, zero once it has passed; `None` when
/// there is no deadline.
pub(crate) fn left_ms(deadline: Option<std::time::Instant>) -> Option<u64> {
    deadline.map(|d| d.saturating_duration_since(std::time::Instant::now()).as_millis() as u64)
}

/// `at`, or `deadline` when that comes first: a window's own end, never past
/// the wall of the work it runs inside.
pub(crate) fn not_past(at: std::time::Instant, deadline: Option<std::time::Instant>) -> std::time::Instant {
    deadline.map_or(at, |deadline| at.min(deadline))
}

/// Whether `deadline` has passed: reached at its instant, never in rounded
/// milliseconds, so a remaining fraction still counts. The one test every
/// deadline check makes.
pub(crate) fn deadline_passed(deadline: Option<std::time::Instant>) -> bool {
    deadline.is_some_and(|at| std::time::Instant::now() >= at)
}

/// A stage boundary or final answer may not cross the whole-run deadline.
pub(crate) fn check_deadline(deadline: Option<std::time::Instant>) -> Result<(), crate::Error> {
    if deadline_passed(deadline) {
        Err(crate::Error::Incomplete("no answer: the time limit was reached".to_owned()))
    } else {
        Ok(())
    }
}

/// Temporarily tighten an engine's deadline, retaining an enclosing deadline
/// when this attempt has none or asks for more time. Other limits survive.
pub(crate) fn deadline_scope(
    eng: &tididi::Engine,
    deadline: Option<std::time::Instant>,
) -> Option<tididi::limits::LimitScope<'_>> {
    use tididi::limits::{StopAt, StopCallback, StopDecision};
    let deadline = deadline?;
    Some(eng.limits().edit(|config| {
        match config.stop_rules().unconditional {
            Some(StopAt::Time(outer)) => config.with_deadline(Some(deadline.min(outer))),
            None => config.with_deadline(Some(deadline)),
            // A standalone compiler caller may already use the work clock.
            // Preserve both threshold slots and its callback, adding the wall
            // through the callback only in this mixed-clock case.
            Some(StopAt::WorkUnits(_)) => {
                let previous = config.stop_callback();
                config.with_stop_callback(Some(StopCallback::new(move |meters, now| {
                    if now >= deadline {
                        StopDecision::Stop
                    } else {
                        previous.as_ref().map_or(StopDecision::Continue, |cb| cb.decide(meters, now))
                    }
                })))
            }
        }
    }))
}

/// A sub-step's policy budget cut to what is left of `deadline` at `now`;
/// `None` once nothing is left.
///
/// The budgets cut this way are Arjun reductions'. A policy reads the run's
/// declared time limit and cannot see how much of it is left when the stage
/// is reached, and the reducer polls no deadline: it stops at its own budget
/// and nothing else can cut it. So every caller sizing a reduction cuts the
/// policy here, and declines the reduction on `None`.
pub(crate) fn clamped_to_deadline_ms(
    policy_ms: u64,
    deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> Option<u64> {
    let left_ms = deadline.map(|d| d.saturating_duration_since(now).as_millis() as u64);
    let allowed = left_ms.map_or(policy_ms, |left| policy_ms.min(left));
    (allowed > 0).then_some(allowed)
}

/// Arjun-reduction budget: 1/6 of `--time-limit-ms` for walls up to 300 s and
/// 1/4 above, clamped to `[5_000, 600_000]`; `abs_default_ms` when there is no
/// time limit.
pub(crate) fn arjun_budget_ms(time_limit_ms: Option<u64>, abs_default_ms: u64) -> u64 {
    let short_window = time_limit_ms.is_some_and(|t| t <= 300_000);
    let ratio = if short_window { 1.0 / 6.0 } else { 0.25 };
    resolve_scaled(time_limit_ms, ratio, abs_default_ms, 5_000, 600_000)
}

/// The only place a program deadline is turned into a vtree-construction
/// deadline, starting now. Every call site that knows the wall calls this
/// rather than deriving a construction deadline of its own. The share is a
/// third of the remaining wall clamped to `[90 s, 900 s]` — TiDiDi's safety
/// net over the complete Vitri construction call.
///
/// `None` in ⇒ `None` out: a run with no wall limit gets no construction
/// deadline. The result is additionally clamped to `program_deadline` — the
/// 90 s floor can exceed a short remaining wall, and a construction deadline
/// past the wall would be meaningless.
pub(crate) fn vtree_deadline(program_deadline: Option<std::time::Instant>) -> Option<std::time::Instant> {
    let pd = program_deadline?;
    let now = std::time::Instant::now();
    let remaining_ms = pd.saturating_duration_since(now).as_millis() as u64;
    let budget = std::time::Duration::from_millis((remaining_ms / 3).clamp(90_000, 900_000));
    Some(not_past(now + budget, program_deadline))
}

thread_local! {
    /// Total wall spent constructing vtrees on this thread since the run
    /// started, and how many constructions ran.
    ///
    /// The single source for the reported `vtree_ms`. Standalone constructions
    /// all pass through `vtree`, which wraps its one Vitri call in
    /// [`timed_vtree_build`]; a whole-run frontend, which interleaves
    /// preprocessing and construction, adds Vitri's own construction time through
    /// [`record_vtree_build_ms`]. Timing at those two points rather than at call
    /// sites keeps the total complete wherever the vtrees get built.
    static VTREE_BUILD_NANOS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static VTREE_BUILD_CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Zero the vtree-construction accumulator; `driver::solve` does as a run
/// starts and ends.
pub(crate) fn reset_vtree_build_ms() {
    VTREE_BUILD_NANOS.set(0);
    VTREE_BUILD_CALLS.set(0);
}

/// Vtree-construction wall so far, or `None` when nothing built a vtree.
///
/// `None` is not zero, and must not be reported as zero. A printed 0 is
/// indistinguishable from a timer that missed the work, so callers omit the
/// field entirely on `None`,
/// leaving `Some(0)` to carry the honest "built, and it took under a
/// millisecond".
pub(crate) fn vtree_build_ms() -> Option<u128> {
    if VTREE_BUILD_CALLS.get() == 0 {
        return None;
    }
    Some(VTREE_BUILD_NANOS.get() as u128 / 1_000_000)
}

/// The clock behind the record's `compile_ms`, on every route.
///
/// The record gives each phase its own key — `preprocess_ms`, `arjun_ms`,
/// `vtree_ms`, `compile_ms` — and each key excludes the others. A compile
/// stage can build vtrees inside it (a component's, a cofactor's), and on the
/// model count's cascade it also runs the preprocessing and Arjun it reports
/// separately, so the clock takes both out: the vtree construction billed to
/// the accumulator since it started, and the phase time its caller names.
pub(crate) struct CompileClock {
    started: std::time::Instant,
    vtree_nanos: u64,
}

impl CompileClock {
    /// Start timing a compile stage now.
    pub(crate) fn start() -> Self {
        CompileClock {
            started: std::time::Instant::now(),
            vtree_nanos: VTREE_BUILD_NANOS.get(),
        }
    }

    /// Milliseconds since [`CompileClock::start`], less the vtree construction
    /// since then and less `other_phases_ms`, the time the caller reports under
    /// another key.
    pub(crate) fn compile_ms(&self, other_phases_ms: u64) -> u128 {
        let vtree_nanos = VTREE_BUILD_NANOS.get().saturating_sub(self.vtree_nanos);
        self.started
            .elapsed()
            .as_millis()
            .saturating_sub(vtree_nanos as u128 / 1_000_000)
            .saturating_sub(other_phases_ms as u128)
    }
}

/// Bill a construction whose owner, Vitri, already measured its wall.
///
/// Vitri's whole-run frontend interleaves preprocessing and construction, so
/// wrapping the whole call would charge preprocessing to `vtree_ms`. Its
/// returned `construction_ms` is instead added to this same accumulator. A
/// standalone build remains wrapped by [`timed_vtree_build`] and must not call
/// this as well.
pub(crate) fn record_vtree_build_ms(ms: u64) {
    bill_vtree_build(ms.saturating_mul(1_000_000));
}

/// Run `f` and bill its wall to the vtree-construction accumulator.
///
/// `vtree` already wraps every construction it makes; wrapping a call
/// into it again would double-count.
pub(crate) fn timed_vtree_build<T>(f: impl FnOnce() -> T) -> T {
    let t0 = std::time::Instant::now();
    let out = f();
    bill_vtree_build(t0.elapsed().as_nanos() as u64);
    out
}

/// Add one construction of `nanos` to the accumulator.
fn bill_vtree_build(nanos: u64) {
    VTREE_BUILD_NANOS.set(VTREE_BUILD_NANOS.get().wrapping_add(nanos));
    VTREE_BUILD_CALLS.set(VTREE_BUILD_CALLS.get().wrapping_add(1));
}

#[cfg(test)]
#[path = "tests/budget.rs"]
mod tests;
