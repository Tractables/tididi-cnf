//! The run's time: how the driver carves its time limit into the sub-budgets
//! and deadlines it hands down, and the clocks behind the record's `vtree_ms`
//! and `compile_ms`.
//!
//! The budgets are the compiler's policies, since it knows the run's time
//! limit and the stages that need a share of it; vitri receives the resulting
//! deadline and divides it inside a construction itself.

use std::time::{Duration, Instant};

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
pub(crate) fn left_ms(deadline: Option<Instant>) -> Option<u64> {
    deadline.map(|d| d.saturating_duration_since(Instant::now()).as_millis() as u64)
}

/// `at`, or `deadline` when that comes first: a window's own end, never past
/// the wall of the work it runs inside.
pub(crate) fn not_past(at: Instant, deadline: Option<Instant>) -> Instant {
    deadline.map_or(at, |deadline| at.min(deadline))
}

/// Whether `deadline` has passed: reached at its instant, never in rounded
/// milliseconds, so a remaining fraction still counts. The one test every
/// deadline check makes.
pub(crate) fn deadline_passed(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|at| Instant::now() >= at)
}

/// A stage boundary or final answer may not cross the whole-run deadline.
pub(crate) fn check_deadline(deadline: Option<Instant>) -> Result<(), crate::Error> {
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
    deadline: Option<Instant>,
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
    deadline: Option<Instant>,
    now: Instant,
) -> Option<u64> {
    let left_ms = deadline.map(|d| d.saturating_duration_since(now).as_millis() as u64);
    let allowed = left_ms.map_or(policy_ms, |left| policy_ms.min(left));
    (allowed > 0).then_some(allowed)
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
pub(crate) fn vtree_deadline(program_deadline: Option<Instant>) -> Option<Instant> {
    let pd = program_deadline?;
    let now = Instant::now();
    let remaining_ms = pd.saturating_duration_since(now).as_millis() as u64;
    let budget = Duration::from_millis((remaining_ms / 3).clamp(90_000, 900_000));
    Some(not_past(now + budget, program_deadline))
}

/// The time a run spends building vtrees, the record's `vtree_ms`, and the
/// instructions it spends in the vitri calls that build them, the record's
/// `vtree_instructions`.
///
/// The run's `Config` holds one, so every stage reaches it. A stage that
/// builds a vtree times the build with [`VtreeTime::time`]. A frontend, which
/// interleaves preprocessing and construction in one vitri call, counts the
/// call with [`VtreeTime::count`] and adds the construction time vitri
/// reports with [`VtreeTime::add`]; its preprocessing is in the count.
#[derive(Debug, Default)]
pub(crate) struct VtreeTime {
    spent: std::cell::Cell<Option<Duration>>,
    instructions: InstructionTally,
}

impl VtreeTime {
    /// Run the construction `build`, adding its wall-clock time and counting
    /// its instructions.
    pub(crate) fn time<T>(&self, build: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let built = self.count(build);
        self.add(started.elapsed());
        built
    }

    /// Run a vitri call that builds a vtree and reports its construction time
    /// itself, counting its instructions.
    pub(crate) fn count<T>(&self, call: impl FnOnce() -> T) -> T {
        self.instructions.count(call)
    }

    /// Add one construction that took `spent`.
    pub(crate) fn add(&self, spent: Duration) {
        self.spent.set(Some(self.spent.get().unwrap_or_default() + spent));
    }

    /// The user-mode instructions counted so far; `None` when nothing was
    /// counted, or when the counter could not be opened or read for some
    /// call (`Some` is never a partial sum).
    pub(crate) fn instructions(&self) -> Option<u64> {
        self.instructions.total()
    }

    /// The time spent so far, or `None` when nothing built a vtree.
    ///
    /// `None` is not zero: the record leaves `vtree_ms` out rather than print
    /// a 0 that would read like a timer that missed the work, so `Some(0)`
    /// carries "built, in under a millisecond".
    pub(crate) fn total(&self) -> Option<Duration> {
        self.spent.get()
    }
}

/// A sum of user-mode instructions over calls, read from a Linux perf counter
/// on this thread opened at the first call. Unlike the run's time, the count
/// of a call does not depend on the machine's load, so a benchmark can take a
/// time-budgeted phase out of a whole-process count.
#[derive(Debug, Default)]
struct InstructionTally {
    counter: std::cell::OnceCell<Option<counter::Counter>>,
    sum: std::cell::Cell<Option<u64>>,
    lost: std::cell::Cell<bool>,
}

impl InstructionTally {
    fn count<T>(&self, call: impl FnOnce() -> T) -> T {
        let counter = self.counter.get_or_init(counter::Counter::open).as_ref();
        let before = counter.and_then(counter::Counter::read);
        let result = call();
        match (before, counter.and_then(counter::Counter::read)) {
            (Some(before), Some(after)) => {
                self.sum.set(Some(self.sum.get().unwrap_or(0) + after.saturating_sub(before)));
            }
            _ => self.lost.set(true),
        }
        result
    }

    fn total(&self) -> Option<u64> {
        if self.lost.get() { None } else { self.sum.get() }
    }
}

#[cfg(all(target_os = "linux", target_endian = "little"))]
mod counter {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    /// `struct perf_event_attr` up to `config2` (`PERF_ATTR_SIZE_VER1`); the
    /// kernel takes the fields after it as zero. The bit-field word `flags`
    /// is laid out low bit first, which holds on little-endian targets only.
    #[repr(C)]
    struct Attr {
        kind: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        config1: u64,
        config2: u64,
    }

    const PERF_TYPE_HARDWARE: u32 = 0;
    const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
    const READ_TIME_ENABLED_AND_RUNNING: u64 = 0b11;
    const EXCLUDE_KERNEL_AND_HV: u64 = (1 << 5) | (1 << 6);

    #[derive(Debug)]
    pub(super) struct Counter(OwnedFd);

    impl Counter {
        /// `None` where the kernel refuses the counter (no PMU, a sandbox,
        /// `perf_event_paranoid` above 2).
        pub(super) fn open() -> Option<Counter> {
            let attr = Attr {
                kind: PERF_TYPE_HARDWARE,
                size: std::mem::size_of::<Attr>() as u32,
                config: PERF_COUNT_HW_INSTRUCTIONS,
                sample_period: 0,
                sample_type: 0,
                read_format: READ_TIME_ENABLED_AND_RUNNING,
                flags: EXCLUDE_KERNEL_AND_HV,
                wakeup_events: 0,
                bp_type: 0,
                config1: 0,
                config2: 0,
            };
            // SAFETY: `attr` is a valid `perf_event_attr` of the size it
            // declares; pid 0 and cpu -1 count this thread on any CPU.
            let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, &attr as *const Attr, 0, -1, -1, 0) };
            // SAFETY: a non-negative return is a new descriptor this owns.
            (fd >= 0).then(|| Counter(unsafe { OwnedFd::from_raw_fd(fd as i32) }))
        }

        /// The count so far; `None` when the read fails or the counter was
        /// off the PMU for part of its enabled time (multiplexed), since the
        /// count is then an estimate.
        pub(super) fn read(&self) -> Option<u64> {
            let mut value = [0u64; 3];
            // SAFETY: reads at most the 24 bytes `value` holds.
            let n = unsafe { libc::read(self.0.as_raw_fd(), value.as_mut_ptr().cast(), 24) };
            let [count, enabled, running] = value;
            (n == 24 && enabled == running).then_some(count)
        }
    }
}

#[cfg(not(all(target_os = "linux", target_endian = "little")))]
mod counter {
    #[derive(Debug)]
    pub(super) struct Counter;

    impl Counter {
        pub(super) fn open() -> Option<Counter> {
            None
        }

        pub(super) fn read(&self) -> Option<u64> {
            None
        }
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
    started: Instant,
    vtree: &'a VtreeTime,
    vtree_at_start: Duration,
}

impl<'a> CompileClock<'a> {
    /// Start timing a compile stage now, against the run's `vtree` time.
    pub(crate) fn start(vtree: &'a VtreeTime) -> Self {
        CompileClock {
            started: Instant::now(),
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
