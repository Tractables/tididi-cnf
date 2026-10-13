use super::*;

/// The projection-minimizing reduce takes a size-adaptive share of the
/// declared wall: 38%, up to the 300 s ceiling, and 50% on a formula of
/// a million variables or more.
#[test]
fn the_projected_arjun_reduce_takes_a_size_adaptive_share_of_the_wall() {
    assert_eq!(pmc_arjun_budget_ms(Some(3_600_000), 1_000), 300_000);
    assert_eq!(pmc_arjun_budget_ms(Some(120_000), 1_000), 45_600);
    assert_eq!(pmc_arjun_budget_ms(Some(120_000), 2_000_000), 60_000);
    // No wall declared ⇒ the flat default.
    assert_eq!(pmc_arjun_budget_ms(None, 1_000), 300_000);
}
