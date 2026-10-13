use super::*;
use std::time::{Duration, Instant};

#[test]
fn no_retry_without_a_deadline_or_below_the_floor() {
    assert_eq!(support_enum_arjun_retry_ms(None), None);
    assert_eq!(support_enum_arjun_retry_ms(Some(1_799_999)), None);
}

#[test]
fn retry_is_capped_at_the_ceiling_on_a_full_wall() {
    // 3_600_000 left ⇒ reserve 900k ⇒ available 2_700_000 ⇒ /3 = 900k, capped.
    assert_eq!(
        support_enum_arjun_retry_ms(Some(3_600_000)),
        Some(SUPPORT_ARJUN_MAX_MS)
    );
}

#[test]
fn retry_at_the_floor_still_leaves_the_stage_its_entry_bar() {
    // 1_800_000 left ⇒ reserve 450k ⇒ available 1_350_000 ⇒ /3 = 450k.
    let budget = support_enum_arjun_retry_ms(Some(1_800_000)).unwrap();
    assert_eq!(budget, 450_000);
    // After the retry the stage plans against what is left, and must still
    // clear its own 600s enumeration-window bar.
    let after = 1_800_000 - budget;
    let enum_wall = after - super::shannon_split_reserve_ms(after);
    assert!(enum_wall >= 600_000, "stage would be starved: {enum_wall}");
}

// `unreduced_retry_window` sizes the unreduced retry's window. Four regimes:
// armed (what is left less the Shannon split reserve, floored at the
// reserved tail), short-window (remaining/4), sub-2s tail (None),
// no-wall-limit (250s).
#[test]
fn armed_window_is_what_is_left_less_the_cascade_reserve() {
    let now = Instant::now();
    // Entered with 3600s left: the cascade is owed a quarter (900s) and the
    // stage takes the other 2700s, not the reserved 250s tail.
    let pd = now + Duration::from_secs(3600);
    let adl = pd - UNREDUCED_RETRY_RESERVE;
    assert_eq!(
        unreduced_retry_window(Some(adl), Some(pd), now),
        Some(Duration::from_secs(2700)),
    );
    // Entered late: the primary was cut early and the support-enum stage
    // ahead of this one spent most of the hour, so 740s are left. A quarter
    // of that is under the cascade's 300s floor, so the
    // floor is what the cascade keeps and the stage takes 440s.
    let pd = now + Duration::from_secs(740);
    let adl = pd - UNREDUCED_RETRY_RESERVE;
    assert_eq!(
        unreduced_retry_window(Some(adl), Some(pd), now),
        Some(Duration::from_secs(440)),
    );
}

#[test]
fn armed_window_never_falls_under_the_reserved_tail() {
    let now = Instant::now();
    // With 300s left the cascade's floor would take all of it. The tail the
    // primary deadline reserved is a floor on this stage's window, so it
    // still gets it.
    let pd = now + Duration::from_secs(300);
    let adl = pd - UNREDUCED_RETRY_RESERVE;
    assert_eq!(
        unreduced_retry_window(Some(adl), Some(pd), now),
        Some(UNREDUCED_RETRY_RESERVE),
    );
}

#[test]
fn the_sub_two_second_gate_skips_the_retry() {
    // Under 8 s left skips, 8 s exactly is allowed.
    let now = Instant::now();
    assert_eq!(
        unreduced_retry_window(None, Some(now + Duration::from_secs(4)), now),
        None,
    );
    assert!(unreduced_retry_window(None, Some(now + Duration::from_secs(8)), now).is_some());
}

#[test]
fn no_deadline_reserves_the_fixed_tail() {
    let now = Instant::now();
    // No wall limit, armed or not -> the fixed reserved tail.
    assert_eq!(unreduced_retry_window(None, None, now), Some(UNREDUCED_RETRY_RESERVE));
    assert_eq!(unreduced_retry_window(Some(now), None, now), Some(UNREDUCED_RETRY_RESERVE));
}

// `no_sbva_retry_window` sizes the no-SBVA retry's window. Four regimes:
// declared-wall quarter, capped-at-remaining, sub-minimum (None),
// no-declared-wall (`UNREDUCED_RETRY_RESERVE`) — one more step of the same
// window order as `unreduced_retry_window` above.
#[test]
fn declared_wall_returns_a_quarter_of_the_declared_total() {
    let now = Instant::now();
    // 1000s declared wall, none of it consumed yet -> reserve = 0.25 * 1000s,
    // sized off the declared total, not the (here, identical) remaining wall.
    let pd = now + Duration::from_secs(1000);
    assert_eq!(
        no_sbva_retry_window(Some(1_000_000), Some(pd), now),
        Some(Duration::from_secs(250)),
    );
}

#[test]
fn declared_wall_is_capped_at_whatever_actually_remains() {
    let now = Instant::now();
    // Same 1000s declared wall (quarter = 250s), but only 40s of it is still
    // left -> capped at the remaining wall, not the declared-total quarter.
    let pd = now + Duration::from_secs(40);
    assert_eq!(
        no_sbva_retry_window(Some(1_000_000), Some(pd), now),
        Some(Duration::from_secs(40)),
    );
}

#[test]
fn window_under_the_minimum_returns_none() {
    let now = Instant::now();
    // 100s declared wall -> quarter = 25s, under `NO_SBVA_RETRY_MIN_WINDOW` (30s)
    // -> skip the stage entirely.
    let pd = now + Duration::from_secs(100);
    assert_eq!(no_sbva_retry_window(Some(100_000), Some(pd), now), None);
    // Exactly at the 30s boundary: 120s declared wall -> quarter = 30s -> allowed.
    let pd2 = now + Duration::from_secs(120);
    assert_eq!(
        no_sbva_retry_window(Some(120_000), Some(pd2), now),
        Some(NO_SBVA_RETRY_MIN_WINDOW),
    );
}

#[test]
fn no_declared_wall_returns_the_fixed_tail_reserve() {
    let now = Instant::now();
    // No declared wall (the deadline is derived from it, so the two are
    // absent together) -> the fixed `UNREDUCED_RETRY_RESERVE`, same as the no-wall-
    // limit regime of `unreduced_retry_window` above.
    assert_eq!(no_sbva_retry_window(None, None, now), Some(UNREDUCED_RETRY_RESERVE));
}
