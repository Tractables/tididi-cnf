use super::pwmc_preprocess_config;
use std::time::{Duration, Instant};

/// The Arjun window is the flat 300 s policy when the deadline has room,
/// what is left when it has less, and off once it is spent.
#[test]
fn pwmc_preprocess_uses_the_callers_exact_arjun_window_once() {
    let parent = crate::test_support::minimal_config();
    let now = Instant::now();
    let roomy = pwmc_preprocess_config(&parent, Some(now + Duration::from_secs(900)), now);
    assert_eq!(roomy.arjun_budget, vitri::config::ArjunBudget::Exact(Duration::from_millis(300_000)));

    let deadline = now + Duration::from_millis(45_000);
    let config = pwmc_preprocess_config(&parent, Some(deadline), now);

    assert_eq!(config.budget_ms, None, "do not derive a second stage share");
    assert_eq!(config.deadline, Some(deadline));
    assert_eq!(
        config.arjun_budget,
        vitri::config::ArjunBudget::Exact(Duration::from_millis(45_000)),
    );
    assert!(config.stages.arjun);

    let spent = now - Duration::from_millis(1);
    let config = pwmc_preprocess_config(&parent, Some(spent), now);
    assert_eq!(config.budget_ms, None);
    assert_eq!(config.deadline, Some(spent));
    assert_eq!(config.arjun_budget, vitri::config::ArjunBudget::Derived);
    assert!(!config.stages.arjun);
    assert!(config.stages.simplify, "the rest of preprocessing stays on");
}
