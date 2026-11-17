use super::*;
use std::time::{Duration, Instant};

/// A sub-step's policy budget is cut to what is left of the deadline its
/// caller carved out; nothing left is no budget, and no deadline leaves the
/// policy standing.
#[test]
fn a_policy_budget_is_cut_to_the_deadline() {
    let now = Instant::now();
    let in_ms = |ms| Some(now + Duration::from_millis(ms));
    assert_eq!(clamped_to_deadline_ms(300_000, in_ms(176_000), now), Some(176_000));
    assert_eq!(clamped_to_deadline_ms(300_000, in_ms(400_000), now), Some(300_000));
    assert_eq!(clamped_to_deadline_ms(300_000, in_ms(0), now), None);
    assert_eq!(clamped_to_deadline_ms(300_000, Some(now - Duration::from_millis(1)), now), None);
    assert_eq!(clamped_to_deadline_ms(300_000, None, now), Some(300_000));
}

/// A window ends at its own end or at the enclosing deadline, whichever comes
/// first; without a deadline, at its own.
#[test]
fn a_window_never_ends_past_its_deadline() {
    let now = Instant::now();
    let soon = now + Duration::from_secs(20);
    let later = now + Duration::from_secs(30);
    assert_eq!(not_past(later, Some(soon)), soon);
    assert_eq!(not_past(soon, Some(later)), soon);
    assert_eq!(not_past(later, None), later);
}

/// A run without a time limit gives the vtree construction no deadline.
#[test]
fn deadline_none_without_a_wall() {
    assert!(vtree_deadline(None).is_none());
}

/// On a long time limit the construction deadline is its capped share, from
/// now.
#[test]
fn deadline_is_now_plus_budget_when_the_wall_is_long() {
    let before = Instant::now();
    let pd = before + Duration::from_secs(3600);
    let d = vtree_deadline(Some(pd)).expect("wall present");
    let after = Instant::now();
    // A third of the remaining wall, capped at 900 s, from the clock reading
    // taken inside the call.
    assert!(before + Duration::from_secs(900) <= d && d <= after + Duration::from_secs(900));
}

/// The construction deadline never falls past the run's own.
#[test]
fn deadline_never_exceeds_the_program_deadline() {
    // 60 s left: the floor (90 s) would overshoot the wall, so we clamp.
    let pd = Instant::now() + Duration::from_secs(60);
    assert_eq!(vtree_deadline(Some(pd)), Some(pd));
}

/// A counted call adds no time, and where the kernel grants the perf counter
/// its instructions are in the count; where it does not, the count is absent
/// rather than zero.
#[test]
fn a_counted_call_adds_its_instructions_and_no_time() {
    let time = VtreeTime::default();
    assert_eq!(time.instructions(), None);
    time.count(|| (0..1_000_000u64).fold(0, |acc, x| std::hint::black_box(acc ^ x)));
    assert_eq!(time.total(), None);
    if let Some(counted) = time.instructions() {
        assert!(counted >= 1_000_000, "a million iterations counted as {counted} instructions");
    }
}

/// A run's vtree time is absent until something is built, and then counts
/// even a build too quick to reach a millisecond.
#[test]
fn vtree_time_is_absent_until_a_build() {
    let time = VtreeTime::default();
    assert_eq!(time.total(), None);
    time.time(|| ());
    assert!(time.total().is_some());
    time.add(Duration::from_millis(5));
    assert!(time.total() >= Some(Duration::from_millis(5)));
}

/// The compile clock leaves out the vtree construction added since it
/// started, and only that.
#[test]
fn the_compile_clock_leaves_out_vtree_time() {
    let time = VtreeTime::default();
    time.add(Duration::from_secs(3600));
    let clock = CompileClock::start(&time);
    std::thread::sleep(Duration::from_millis(20));
    assert!(clock.compile_ms(0) >= 20, "construction before the clock started is not taken out");
    time.add(Duration::from_secs(3600));
    assert_eq!(clock.compile_ms(0), 0, "construction since the clock started is taken out");
}

/// A budgeted call that contains a counted one is counted once, and a call
/// counted only when it stops keeps what the calls nested in it counted.
#[test]
fn nested_budgeted_calls_are_counted_once() {
    let spin = || (0..1_000_000u64).fold(0, |acc, x| std::hint::black_box(acc ^ x));
    let nested = VtreeTime::default();
    nested.budgeted(|| nested.count(spin));
    let alone = VtreeTime::default();
    alone.count(spin);
    if let (Some(nested), Some(alone)) = (nested.budgeted_instructions(), alone.budgeted_instructions()) {
        assert!(nested < alone * 2, "the inner count was added to the outer one: {nested} against {alone}");
    }

    let finished = VtreeTime::default();
    finished.budgeted_if(|| finished.budgeted(spin), |_| false);
    assert_eq!(finished.budgeted_instructions().is_some(), alone.budgeted_instructions().is_some());
    let skipped = VtreeTime::default();
    skipped.budgeted_if(spin, |_| false);
    assert_eq!(skipped.budgeted_instructions(), None);
}

/// The run's hashes name its compile inputs: absent before a compile, equal
/// for equal inputs and different when the vtree differs.
#[test]
fn compile_hashes_follow_the_inputs() {
    use crate::test_support::{formula, lit};
    let formula = formula(4, vec![vitri::cnf::Clause::new(vec![lit(0, true), lit(3, false)])]);
    let balanced = tididi::vtree::Vtree::balanced(4);
    let order: Vec<_> = (1..=4).map(tididi::vtree::VarId).collect();
    let linear = tididi::vtree::Vtree::linear_from_order(&order).unwrap();
    let [a, b, c] = [&balanced, &balanced, &linear].map(|vtree| {
        let work = VtreeTime::default();
        assert_eq!(work.compiled_hashes(), (None, None));
        work.note_compile(&formula, vtree);
        work.compiled_hashes()
    });
    assert_eq!(a, b);
    assert_eq!(a.0, c.0);
    assert_ne!(a.1, c.1);
}
