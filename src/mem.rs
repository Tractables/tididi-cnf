//! Process-wide jemalloc allocator and heap accounting.
//!
//! Two readings, both process-wide and refreshed by an `epoch.advance()`
//! first. `mapped_bytes` (`stats.mapped + stats.retained`) is the address
//! space that `RLIMIT_AS`, lowered for a run by its memory limit, charges; the
//! library's memory hooks and the recovery log read it. `stats.allocated`,
//! which the recovery log prints beside it, is the live working set. The
//! per-thread `thread.allocated` counters are not used: a buffer allocated on
//! one thread and freed on another inflates the first thread's net figure
//! without bound.
//!
//! A read is costly, nearly all of it in `epoch.advance`, so the library's
//! per-apply checks go through the throttle in `preflight_alloc_decay_check`,
//! and no check runs per node.
//!
//! Nothing may panic inside `GlobalAlloc::alloc`: `__rust_alloc` is
//! `#[rustc_nounwind]`. A refused allocation panics at its call site through
//! Rust's default handler instead, which is what lets
//! the model count's recovery (`stage/mc/recovery.rs`) catch it.

use tikv_jemalloc_ctl::{epoch, epoch_mib, raw, stats};
use tikv_jemalloc_ctl::stats::{allocated_mib, mapped_mib, retained_mib};

use std::cell::Cell;

/// jemalloc's `MALLCTL_ARENAS_ALL` sentinel, as `<jemalloc/jemalloc_macros.h>`
/// defines it: used as the arena index of an `arena.<i>.purge` mallctl write,
/// it applies the purge to every initialized arena.
const MALLCTL_ARENAS_ALL: usize = 4096;

thread_local! {
    static EPOCH_HANDLE: Cell<Option<epoch_mib>> = const { Cell::new(None) };
    static ALLOC_HANDLE: Cell<Option<allocated_mib>> = const { Cell::new(None) };
    static MAPPED_HANDLE: Cell<Option<mapped_mib>> = const { Cell::new(None) };
    static RETAINED_HANDLE: Cell<Option<retained_mib>> = const { Cell::new(None) };
}

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// jemalloc startup config: `retain:false` + decay disabled (lazy mode).
///
/// **`retain:false`** — freed chunks are munmap'd instead of parked in the
/// retained pool. Under `ulimit -v` (set by `--mem-limit-mb`, or by whatever
/// starts the process), retained chunks count against VSZ but cannot be
/// reused for fresh allocations that exceed jemalloc's per-arena
/// fragmentation pattern. Without this, a failed compile's freed memory stays
/// in the address-space budget until process exit, leaving the recovery
/// cascade no room for its re-Arjun'd sub-formulas.
///
/// **`dirty_decay_ms:-1,muzzy_decay_ms:-1`** — never purge dirty pages on a
/// timer; keep them in the arena for reuse. The apply/contract cycle frees
/// and reallocates level-sized buffers constantly, and pages handed back to
/// the OS are faulted in again: with zero decay a large compile faults in
/// several times its peak memory and spends a large share of its time in the
/// kernel. A decay of a second or ten saves almost nothing, since the freed
/// pages come back into use later than that. Cost: `mapped+retained` tracks
/// the high-water mark instead of live bytes, and the held pages count
/// against `RLIMIT_AS`. [`preflight_alloc_decay_check`] switches to eager
/// reclamation ([`reclaim_eagerly`]) when `mapped_bytes()` reaches
/// [`LAZY_FLIP_FRAC`] of `RLIMIT_AS`. The OOM-recovery cascade
/// (`stage/mc/recovery.rs`) purges after every failed compile and leaves the
/// decay setting as it is.
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
// Safety: jemalloc reads `malloc_conf` as a thin `const char *` (a `&[u8]`
// slice is a fat pointer and would mis-link). The union reinterprets a
// reference to the first byte of a null-terminated byte-string literal as
// `*const libc::c_char`; both are pointer-sized with identical
// representation, and the literal has static lifetime. This is the idiom
// from the `background_thread_enabled` test in tikv-jemallocator's own
// suite.
pub(crate) static malloc_conf: Option<&'static libc::c_char> = Some(unsafe {
    union U { x: &'static u8, y: &'static libc::c_char }
    U { x: &b"retain:false,dirty_decay_ms:-1,muzzy_decay_ms:-1\0"[0] }.y
});

/// One-way latch: `true` once the process has switched from lazy page
/// reclamation (startup `dirty_decay_ms:-1`) to eager (`decay 0` + purge).
/// Set by [`reclaim_eagerly`], so the pressure check below makes the mallctl
/// writes once.
static EAGER_DECAY_LATCHED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Fraction of `RLIMIT_AS` at which lazy decay switches to eager.
///
/// Against an earlier 40%, compiles whose footprint passes 40% of the cap but
/// stays under 60% run about a tenth faster, with fewer page faults and more
/// peak memory. A 50% line gave up most of that gain, and purging at 60%
/// while leaving decay off afterwards made the allocator refuse requests that
/// the switch lets through.
const LAZY_FLIP_FRAC: f64 = 0.60;

thread_local! {
    /// Bytes of tracked capacity growth since this thread last did a real
    /// (mapped vs rlimit) pressure read, plus `LAZY_POLL_CHARGE` per
    /// request-0 heartbeat poll. Bounds flip overshoot to
    /// `LAZY_CHECK_STEP` per thread without paying an epoch advance on
    /// every realloc or every apply entry.
    static LAZY_CHECK_ACCUM: Cell<u64> = const { Cell::new(0) };
}

/// Tracked-growth bytes between real pressure reads (per thread).
const LAZY_CHECK_STEP: u64 = 64 * 1024 * 1024;

/// Upper bound on consecutive request-0 heartbeat polls that may pass without
/// a real pressure read, when no sized allocation request intervenes.
///
/// A real read is dominated by jemalloc's `epoch.advance`. The worst case is a
/// run of very many small component compiles, each entering several applies:
/// a read per entry costs a visible share of each compile's wall, and one per
/// this many polls makes it negligible. A larger bound buys little and
/// stretches the time to notice the flip.
const LAZY_POLLS_PER_CHECK: u64 = 64;

/// Notional growth charged to a request-0 poll so that polls and sized
/// requests share the one accumulator instead of needing a second throttle:
/// [`LAZY_POLLS_PER_CHECK`] polls alone reach the step, and genuine allocation
/// growth reaches it sooner.
const LAZY_POLL_CHARGE: u64 = LAZY_CHECK_STEP / LAZY_POLLS_PER_CHECK;

/// Pure throttle decision for [`preflight_alloc_decay_check`], factored out so
/// the bound on polls-between-real-reads is unit-testable without touching
/// process-global jemalloc state.
///
/// Given the per-thread accumulator (tracked growth since this thread's last
/// real read) and a pending request, returns `(new_accum, due)`:
///
/// * `request_bytes >= LAZY_CHECK_STEP` — a single request big enough to move
///   the needle on its own: always due, accumulator untouched (the request is
///   not "growth since the last read", it is the read's reason to happen).
/// * `request_bytes == 0` — a heartbeat poll, which the library makes once per
///   top-level apply through the reclaim hook `driver::solve_on` installs:
///   charged `LAZY_POLL_CHARGE`, so at most `LAZY_POLLS_PER_CHECK`
///   consecutive polls can pass without a real read.
///   They must not bypass the throttle: on a workload of many tiny applies the
///   polls alone cost a measurable slice of the window.
/// * otherwise — charged its own byte count.
fn lazy_check_due(accum: u64, request_bytes: u64) -> (u64, bool) {
    if request_bytes >= LAZY_CHECK_STEP {
        return (accum, true);
    }
    let charge = if request_bytes == 0 { LAZY_POLL_CHARGE } else { request_bytes };
    let n = accum.saturating_add(charge);
    if n >= LAZY_CHECK_STEP { (0, true) } else { (n, false) }
}

/// Pressure check for the lazy-decay startup mode, callable *before* a
/// large allocation. Once jemalloc's `mapped+retained` (plus the pending
/// request) crosses `LAZY_FLIP_FRAC` of `RLIMIT_AS`, permanently switch
/// to eager reclamation ([`reclaim_eagerly`]). The purge runs before the
/// caller's `try_reserve`, so dirty pages are returned to the OS before the
/// kernel charges the new request against `RLIMIT_AS`.
///
/// Why flip below the cap: the library's growth-mode headroom check,
/// installed as a memory hook in `driver::solve_on`, reads `mapped_bytes()`
/// as the address space in use, and the held pages really are charged against
/// `RLIMIT_AS`. Below the line the held pages only shrink that headroom; past
/// it, zero decay keeps the number close to the live footprint while the cap
/// approaches. Between two real reads `mapped+retained` can pass the line by
/// several GiB, which the room above it absorbs.
///
/// Every entry, sized request or heartbeat poll, goes through the per-thread
/// throttle `lazy_check_due` first. No-op once latched, and inert when
/// `RLIMIT_AS` is unlimited (nothing to protect; lazy mode is strictly
/// better).
#[inline]
pub(crate) fn preflight_alloc_decay_check(request_bytes: u64) {
    use std::sync::atomic::Ordering;
    if EAGER_DECAY_LATCHED.load(Ordering::Relaxed) {
        return;
    }
    let due = LAZY_CHECK_ACCUM.with(|c| {
        let (next, due) = lazy_check_due(c.get(), request_bytes);
        c.set(next);
        due
    });
    if !due {
        return;
    }
    let Some(limit) = rlimit_as_bytes() else { return };
    let threshold = (limit as f64 * LAZY_FLIP_FRAC) as u64;
    let mapped = mapped_bytes();
    if mapped.saturating_add(request_bytes) >= threshold {
        eprintln!(
            "[mem] mapped+retained {} MiB (+{} MiB pending) ≥ {}% of rlimit_as {} MiB — switching jemalloc to eager decay",
            mapped / (1024 * 1024),
            request_bytes / (1024 * 1024),
            (LAZY_FLIP_FRAC * 100.0) as u64,
            limit / (1024 * 1024),
        );
        let reclaimed = reclaim_eagerly();
        eprintln!(
            "[mem] decay flip: {:?} mapped+retained {} → {} MiB",
            reclaimed,
            mapped / (1024 * 1024),
            mapped_bytes() / (1024 * 1024),
        );
    }
}

/// Force jemalloc to purge all unused dirty pages from every initialized
/// arena. Issues `arena.<MALLCTL_ARENAS_ALL>.purge` via `mallctl`.
///
/// Used after a compile panics and between recovery cofactors, so the next
/// compile starts with the freed pages returned to the OS rather than held
/// as dirty pages under the address-space cap.
///
/// Best-effort: returns `Ok(())` on success, `Err(_)` if mallctl fails
/// (e.g. jemalloc not initialized yet). Callers should ignore the error
/// — failure to purge is a missed optimization, not a correctness bug.
pub(crate) fn purge_all_arenas() -> Result<(), libc::c_int> {
    let mut mib = [0usize; 3];
    raw::name_to_mib(b"arena.0.purge\0", &mut mib).map_err(|_| libc::ENOENT)?;
    mib[1] = MALLCTL_ARENAS_ALL;
    // `arena.<i>.purge` is a void control: jemalloc requires
    // `(newp == NULL) == (newlen == 0)`, so it must be invoked with all
    // pointers null. `raw::write_mib::<()>(&mib, ())` does not do that —
    // `&mut ()` is a non-null dangling pointer with `newlen == 0`, which
    // jemalloc rejects with `EINVAL` ("attempt to write void value").
    // Safety: `mib` is a well-formed MIB from `name_to_mib` with the
    // documented `MALLCTL_ARENAS_ALL` index substituted; all-null old/new
    // pointers are the documented invocation for void controls.
    let rc = unsafe {
        tikv_jemalloc_sys::mallctlbymib(
            mib.as_ptr(),
            mib.len(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(rc);
    }
    Ok(())
}

/// Switch jemalloc to eager reclamation for the rest of the process: future
/// frees give their pages back at once ([`set_decay_zero`]), and the pages
/// already dirty go back now ([`purge_all_arenas`]).
///
/// Best-effort, like both halves: `Err` carries the first mallctl failure.
fn reclaim_eagerly() -> Result<(), libc::c_int> {
    let decay = set_decay_zero();
    let purge = purge_all_arenas();
    decay.and(purge)
}

/// Set `dirty_decay_ms` and `muzzy_decay_ms` to zero across every
/// initialized arena, so jemalloc reclaims dirty/muzzy pages immediately
/// instead of holding them indefinitely (startup config is decay disabled —
/// see [`malloc_conf`]).
///
/// Also latches `EAGER_DECAY_LATCHED` so [`preflight_alloc_decay_check`]
/// becomes a no-op afterwards, and sets the default that arenas created
/// later start from.
///
/// Best-effort: returns `Ok(())` on success, `Err(_)` if either mallctl
/// write fails.
fn set_decay_zero() -> Result<(), libc::c_int> {
    EAGER_DECAY_LATCHED.store(true, std::sync::atomic::Ordering::Relaxed);
    // The default arenas created after this call start from. The per-arena
    // loop below reaches only arenas that already exist; a later one would
    // otherwise start from the `malloc_conf` default, which is lazy (-1), and
    // never purge.
    // Safety: `arenas.dirty_decay_ms` / `arenas.muzzy_decay_ms` are
    // documented rw `ssize_t` controls; `0` is a valid POD write value.
    unsafe {
        let _ = raw::write(b"arenas.dirty_decay_ms\0", 0 as libc::ssize_t);
        let _ = raw::write(b"arenas.muzzy_decay_ms\0", 0 as libc::ssize_t);
    }
    // Per-arena writes. Unlike `arena.<i>.purge`, the decay-ms controls
    // do not accept `MALLCTL_ARENAS_ALL` — jemalloc's handler does a plain
    // `arena_get(arena_ind)`, gets `NULL` for the sentinel, and returns
    // `EFAULT`. Loop over `arenas.narenas` instead, skipping
    // uninitialized arenas (their write fails with `EFAULT` — harmless;
    // they inherit the `arenas.*` defaults written above when created).
    // Safety: `arenas.narenas` is a documented read-only `unsigned`
    // control; the per-arena strings are valid null-terminated controls
    // and `0: ssize_t` is the documented POD write value.
    let narenas: u32 = unsafe { raw::read(b"arenas.narenas\0") }.map_err(|_| libc::ENOENT)?;
    let mut dirty_mib = [0usize; 3];
    let mut muzzy_mib = [0usize; 3];
    raw::name_to_mib(b"arena.0.dirty_decay_ms\0", &mut dirty_mib).map_err(|_| libc::ENOENT)?;
    raw::name_to_mib(b"arena.0.muzzy_decay_ms\0", &mut muzzy_mib).map_err(|_| libc::ENOENT)?;
    let mut wrote_any = false;
    for i in 0..narenas as usize {
        dirty_mib[1] = i;
        muzzy_mib[1] = i;
        // Safety: well-formed MIBs from `name_to_mib` with a valid arena
        // index substituted; `0: ssize_t` is the documented write type.
        let ok = unsafe {
            let d = raw::write_mib::<libc::ssize_t>(&dirty_mib, 0).is_ok();
            let m = raw::write_mib::<libc::ssize_t>(&muzzy_mib, 0).is_ok();
            d && m
        };
        wrote_any |= ok;
    }
    if wrote_any { Ok(()) } else { Err(libc::EFAULT) }
}

/// The thread's cached jemalloc MIB, looked up by name on first use.
#[inline]
fn cached_mib<T: Copy + 'static>(
    cell: &'static std::thread::LocalKey<Cell<Option<T>>>,
    lookup: impl FnOnce() -> T,
) -> T {
    cell.with(|c| {
        c.get().unwrap_or_else(|| {
            let m = lookup();
            c.set(Some(m));
            m
        })
    })
}

fn epoch_handle() -> epoch_mib {
    cached_mib(&EPOCH_HANDLE, || epoch::mib().expect("jemalloc epoch mib"))
}

fn alloc_handle() -> allocated_mib {
    cached_mib(&ALLOC_HANDLE, || stats::allocated::mib().expect("jemalloc stats.allocated mib"))
}

fn mapped_handle() -> mapped_mib {
    cached_mib(&MAPPED_HANDLE, || stats::mapped::mib().expect("jemalloc stats.mapped mib"))
}

fn retained_handle() -> retained_mib {
    cached_mib(&RETAINED_HANDLE, || stats::retained::mib().expect("jemalloc stats.retained mib"))
}

/// Total bytes of virtual address space jemalloc has obtained from the OS,
/// computed as `stats.mapped + stats.retained`. This approximates the
/// kernel-side `VmSize` for our process — modulo the binary text, thread
/// stacks, and any non-jemalloc mmap'd regions, all small beside it. It's the
/// figure that `RLIMIT_AS` actually charges against.
///
/// **Why mapped + retained, not just mapped.** `stats.mapped` alone can read
/// well under the limit at the moment the OS refuses a multi-GiB allocation.
/// jemalloc's mapped counter covers extents in *active* use; retained extents
/// (decommitted via `madvise(MADV_DONTNEED)` and held for reuse) are tracked
/// separately, even though they still occupy address space and count against
/// RLIMIT_AS.
/// Their sum reproduces VmSize closely enough for the budget check.
#[inline]
pub(crate) fn mapped_bytes() -> u64 {
    let _ = epoch_handle().advance();
    let m = mapped_handle().read().unwrap_or(0) as u64;
    let r = retained_handle().read().unwrap_or(0) as u64;
    m.saturating_add(r)
}

/// jemalloc's process-wide counters from one epoch advance: the two parts
/// [`mapped_bytes`] sums, with the live allocation beside them. The recovery
/// log prints them around a purge to show whether the footprint is live data
/// (`allocated ≈ mapped`) or address space jemalloc holds for reuse.
/// Diagnostic only; budget checks use [`mapped_bytes`].
#[derive(Clone, Copy)]
pub(crate) struct HeapStats {
    /// `stats.allocated`: bytes in live allocations.
    pub(crate) allocated: u64,
    /// `stats.mapped`: bytes in extents in active use.
    pub(crate) mapped: u64,
    /// `stats.retained`: bytes held for reuse, still in the address space.
    pub(crate) retained: u64,
}

impl HeapStats {
    /// Advance the epoch and read the three counters.
    pub(crate) fn read() -> Self {
        let _ = epoch_handle().advance();
        HeapStats {
            allocated: alloc_handle().read().unwrap_or(0) as u64,
            mapped: mapped_handle().read().unwrap_or(0) as u64,
            retained: retained_handle().read().unwrap_or(0) as u64,
        }
    }
}

/// Current address-space limit (`RLIMIT_AS`), in bytes, or `None` when
/// unlimited or unreadable; callers treat both as no cap. A run's memory
/// limit lowers it while the run lasts ([`limit_address_space`]); competition
/// runs inherit whatever the contest runner imposes.
pub(crate) fn rlimit_as_bytes() -> Option<u64> {
    let rl = getrlimit_as()?;
    (rl.rlim_cur != libc::RLIM_INFINITY).then_some(rl.rlim_cur)
}

/// The current `RLIMIT_AS` soft and hard limits, or `None` when `getrlimit`
/// fails.
fn getrlimit_as() -> Option<libc::rlimit> {
    let mut rl = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // Safety: `rl` is a valid, properly aligned `libc::rlimit` on the
    // stack. `getrlimit` is a standard POSIX syscall that writes into
    // the struct on success and leaves it unchanged on failure; passing
    // a mutable reference satisfies the C API contract.
    (unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut rl) } == 0).then_some(rl)
}

/// A run's memory limit, held for as long as the run lasts: dropping it puts
/// back the `RLIMIT_AS` the run found, so the process that made the run keeps
/// its own limit afterwards.
pub(crate) struct AddressSpaceLimit {
    /// The limits to restore; `None` when the run set none.
    previous: Option<libc::rlimit>,
}

impl Drop for AddressSpaceLimit {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            // Safety: `previous` is the `libc::rlimit` `getrlimit` returned,
            // so `rlim_cur <= rlim_max`; the run lowered only the soft limit,
            // which a process may raise again up to the hard limit. A failure
            // leaves the run's lower limit in place, which is safe.
            unsafe { libc::setrlimit(libc::RLIMIT_AS, previous) };
        }
    }
}

/// Apply a run's memory limit of `mb` MiB until the returned guard drops:
/// lower `RLIMIT_AS` to it through [`set_rlimit_as_bytes`], and warn when that
/// fails, since the memory-adaptive machinery then stays inert. `0` leaves the
/// limit unchanged.
pub(crate) fn limit_address_space(mb: u64) -> Result<AddressSpaceLimit, crate::Error> {
    if mb == 0 {
        return Ok(AddressSpaceLimit { previous: None });
    }
    let previous = getrlimit_as();
    if !set_rlimit_as_bytes(mb.saturating_mul(1024 * 1024)) {
        eprintln!(
            "warning: memory limit of {} MiB: could not lower RLIMIT_AS (setrlimit \
             failed); memory-adaptive machinery may stay inert",
            mb
        );
    }
    Ok(AddressSpaceLimit { previous })
}

/// Lower this process's `RLIMIT_AS` (address-space) soft limit to `limit` bytes.
///
/// This is the run's own equivalent of `ulimit -v`. Its purpose is to make a
/// memory ceiling *visible* to the solver's memory-adaptive machinery, all of
/// which keys off [`rlimit_as_bytes`] (a `getrlimit` syscall): the lazy→eager
/// arena-decay flip in this module, the library's growth-mode headroom check
/// (installed as a memory hook in `driver::solve_on`), and the bail projection
/// in `stage/mc/recovery.rs`. Under a cgroup memory limit the real cap is
/// enforced out-of-band and is *invisible* to `getrlimit`, so without this
/// call those mechanisms stay inert and a bloating compile drives the whole
/// process into an OOM-kill instead of Shannon-splitting. Mirroring the cgroup
/// value here arms them.
///
/// Semantics: **only ever lowers** the soft limit, never raises it — a process
/// may always lower its own soft limit unprivileged, but cannot raise it above
/// the hard limit. So this can never relax a tighter limit the OS/cgroup
/// already imposes; the worst case is a no-op. Returns `true` if the limit is
/// now at or below `limit` (including the no-op case); `false` when
/// `getrlimit` or `setrlimit` fails. `limit` is nonzero, since zero would
/// forbid all allocation: [`limit_address_space`] makes no call for a limit
/// of 0.
fn set_rlimit_as_bytes(limit: u64) -> bool {
    let Some(mut rl) = getrlimit_as() else {
        return false;
    };
    let inf = libc::RLIM_INFINITY as u128;
    match rlimit_as_new_soft(rl.rlim_cur as u128, rl.rlim_max as u128, limit as u128, inf) {
        // Already at or below target (a tighter OS/cgroup-imposed soft limit).
        None => true,
        Some(new_cur) => {
            rl.rlim_cur = new_cur as libc::rlim_t;
            // Safety: `rl` is a valid, properly aligned `libc::rlimit` with
            // `rlim_cur <= rlim_max`; `setrlimit` is a standard POSIX syscall.
            unsafe { libc::setrlimit(libc::RLIMIT_AS, &rl) == 0 }
        }
    }
}

/// Pure decision for [`set_rlimit_as_bytes`] (factored out so it is unit-testable
/// without mutating the process-global RLIMIT_AS, which would be unsafe under the
/// parallel test harness). Given the current soft (`cur`) / hard (`max`) limits
/// and a `target`, returns the new soft limit to install, or `None` if no change
/// is needed. **Only ever lowers** the soft limit, and never above the hard
/// limit. `inf` is the platform's `RLIM_INFINITY` sentinel. Widened to `u128` so
/// the sentinel and large byte counts compare without wraparound on any platform.
fn rlimit_as_new_soft(cur: u128, max: u128, target: u128, inf: u128) -> Option<u128> {
    // Soft limit already finite and at/below target — nothing to do.
    if cur != inf && cur <= target {
        return None;
    }
    // Never set the soft limit above the hard limit (kernel rejects it; we can't
    // raise the hard limit unprivileged anyway). When the hard limit is finite
    // and below target, clamp to it.
    let new_cur = if max != inf && max < target { max } else { target };
    Some(new_cur)
}

#[cfg(test)]
#[path = "tests/mem.rs"]
mod tests;
