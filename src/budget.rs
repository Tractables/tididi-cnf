//! The run's time: how the driver carves its time limit into the sub-budgets
//! and deadlines it hands down, and the clocks behind the record's `vtree_ms`
//! and `compile_ms`.
//!
//! The budgets are the compiler's policies, since it knows the run's time
//! limit and the stages that need a share of it; vitri receives the resulting
//! deadline and divides it inside a construction itself.

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
/// when this attempt has none or asks for more time. Other limits survive,
/// including a work-clock bound a standalone compiler caller armed.
pub(crate) fn deadline_scope(
    eng: &tididi::Engine,
    deadline: Option<std::time::Instant>,
) -> Option<tididi::limits::LimitScope<'_>> {
    let deadline = deadline?;
    Some(eng.limits().edit(|config| config.with_deadline_at_most(deadline)))
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
/// third of the remaining wall clamped to `[90 s, 900 s]`: the compiler's
/// safety net over the complete vitri construction call.
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

/// The time a run spends building vtrees: the record's `vtree_ms`.
///
/// The run's `Config` holds one, so every stage reaches it. A stage that
/// builds a vtree times the build with [`VtreeTime::time`], and a frontend,
/// which interleaves preprocessing and construction in one vitri call, adds
/// the construction time vitri reports with [`VtreeTime::add`].
#[derive(Debug, Default)]
pub(crate) struct VtreeTime(std::cell::Cell<Option<std::time::Duration>>);

impl VtreeTime {
    /// Run the construction `build` and add its wall-clock time.
    pub(crate) fn time<T>(&self, build: impl FnOnce() -> T) -> T {
        let started = std::time::Instant::now();
        let built = build();
        self.add(started.elapsed());
        built
    }

    /// Add one construction that took `spent`.
    pub(crate) fn add(&self, spent: std::time::Duration) {
        self.0.set(Some(self.0.get().unwrap_or_default() + spent));
    }

    /// The time spent so far, or `None` when nothing built a vtree.
    ///
    /// `None` is not zero: the record leaves `vtree_ms` out rather than print
    /// a 0 that would read like a timer that missed the work, so `Some(0)`
    /// carries "built, in under a millisecond".
    pub(crate) fn total(&self) -> Option<std::time::Duration> {
        self.0.get()
    }
}

/// The clock behind the record's `compile_ms`, on every route.
///
/// The record gives each phase its own key — `preprocess_ms`, `arjun_ms`,
/// `vtree_ms`, `compile_ms` — and each key excludes the others. A compile
/// stage can build vtrees inside it (a component's, a cofactor's), and on the
/// model count's cascade it also runs the preprocessing and Arjun it reports
/// separately, so the clock takes both out: the run's vtree time since it
/// started, and the phase time its caller names.
pub(crate) struct CompileClock<'a> {
    started: std::time::Instant,
    vtree: &'a VtreeTime,
    vtree_at_start: std::time::Duration,
}

impl<'a> CompileClock<'a> {
    /// Start timing a compile stage now, against the run's `vtree` time.
    pub(crate) fn start(vtree: &'a VtreeTime) -> Self {
        CompileClock {
            started: std::time::Instant::now(),
            vtree,
            vtree_at_start: vtree.total().unwrap_or_default(),
        }
    }

    /// Milliseconds since [`CompileClock::start`], less the vtree construction
    /// since then and less `other_phases_ms`, the time the caller reports under
    /// another key.
    pub(crate) fn compile_ms(&self, other_phases_ms: u64) -> u128 {
        let vtree = self.vtree.total().unwrap_or_default().saturating_sub(self.vtree_at_start);
        self.started
            .elapsed()
            .as_millis()
            .saturating_sub(vtree.as_millis())
            .saturating_sub(other_phases_ms as u128)
    }
}

#[cfg(test)]
#[path = "tests/budget.rs"]
mod tests;
