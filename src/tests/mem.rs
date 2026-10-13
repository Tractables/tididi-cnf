use super::{lazy_check_due, rlimit_as_new_soft, LAZY_CHECK_STEP, LAZY_POLLS_PER_CHECK, LAZY_POLL_CHARGE};

/// Drive a sequence of requests through the pure throttle, returning the
/// number of real reads it would trigger and the longest run of
/// consecutive throttled (non-reading) calls.
fn replay(requests: impl IntoIterator<Item = u64>) -> (usize, usize) {
    let mut accum = 0u64;
    let mut reads = 0usize;
    let mut run = 0usize;
    let mut worst_run = 0usize;
    for r in requests {
        let (next, due) = lazy_check_due(accum, r);
        accum = next;
        if due {
            reads += 1;
            run = 0;
        } else {
            run += 1;
            worst_run = worst_run.max(run);
        }
    }
    (reads, worst_run)
}

#[test]
fn heartbeat_polls_are_throttled_to_the_documented_bound() {
    // Charged, N polls cost N / `LAZY_POLLS_PER_CHECK` real reads, from a
    // cold accumulator on.
    let per_check = LAZY_POLLS_PER_CHECK as usize;
    let (reads, worst_run) = replay(std::iter::repeat_n(0, per_check * 10));
    assert_eq!(reads, 10, "expected one real read per {per_check} polls");
    assert!(worst_run <= per_check, "at most {per_check} polls may pass between real reads, saw {worst_run}");
}

#[test]
fn sized_requests_reach_the_step_by_bytes() {
    // Growth accumulates by byte count and trips at `LAZY_CHECK_STEP`.
    let mib = 1024 * 1024u64;
    let n = (LAZY_CHECK_STEP / mib) as usize;
    let (reads, worst_run) = replay(std::iter::repeat_n(mib, n));
    assert_eq!(reads, 1);
    assert_eq!(worst_run, n - 1);
}

#[test]
fn sized_growth_shortens_the_poll_interval() {
    // Polls and sized requests share one accumulator, so real allocation
    // growth makes the next poll due sooner — the flip still engages
    // promptly under genuine memory pressure.
    let almost = LAZY_CHECK_STEP - LAZY_POLL_CHARGE;
    let (accum, due) = lazy_check_due(0, almost);
    assert!(!due);
    assert!(lazy_check_due(accum, 0).1, "poll after big growth must read");
}

#[test]
fn oversized_single_request_always_reads_and_keeps_the_accumulator() {
    let (accum, due) = lazy_check_due(123, LAZY_CHECK_STEP);
    assert!(due);
    assert_eq!(accum, 123, "a step-sized request must not reset tracked growth");
}

// A stand-in for `RLIM_INFINITY`; the function treats `inf` purely as an
// opaque sentinel.
const INF: u128 = u128::MAX;
const G: u128 = 1024 * 1024 * 1024;

#[test]
fn lowers_unlimited_soft_to_target() {
    // Competition case: cgroup-enforced cap is invisible, so getrlimit
    // reports unlimited. We install the target.
    assert_eq!(rlimit_as_new_soft(INF, INF, 32 * G, INF), Some(32 * G));
}

#[test]
fn lowers_finite_soft_above_target() {
    assert_eq!(rlimit_as_new_soft(64 * G, 64 * G, 32 * G, INF), Some(32 * G));
}

#[test]
fn never_raises_soft_already_at_or_below_target() {
    // A tighter limit already imposed (e.g. by ulimit/cgroup) is left alone.
    assert_eq!(rlimit_as_new_soft(16 * G, 64 * G, 32 * G, INF), None);
    assert_eq!(rlimit_as_new_soft(32 * G, 32 * G, 32 * G, INF), None);
}

#[test]
fn clamps_to_finite_hard_limit_below_target() {
    // Soft unlimited but hard limit finite and below target: clamp to hard,
    // never exceed it (the kernel would reject, and we can't raise it).
    assert_eq!(rlimit_as_new_soft(INF, 20 * G, 32 * G, INF), Some(20 * G));
}
