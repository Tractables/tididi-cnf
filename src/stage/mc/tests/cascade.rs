/// A cofactor whose vtree cannot be built gives up with `None`, as a compile
/// that gives up does, and the recovery reads it as one. A panic is kept for a
/// broken invariant or a refused allocation, not for input vitri refuses.
#[test]
fn a_cofactor_whose_vtree_cannot_be_built_gives_up_without_panicking() {
    let eng = &tididi::Engine::new();
    let config = crate::test_support::minimal_config();
    let run = crate::test_support::run(eng, &config);
    let no_variables = crate::test_support::formula(0, Vec::new());
    let counted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::count_cofactor(&run, &no_variables)
    }));
    assert!(matches!(counted, Ok(None)));
}

/// A primary compile that stalls is given up and the same reduction is
/// compiled again on a `goatd-primal` tree: the count is the formula's, the
/// stall recompile answers it, and the run names the recompile's tree. The
/// run's own rule, a ten-minute wall at an hour's limit, never fires on a
/// formula this small; a zero wall on every step fires on the first.
#[test]
fn a_stalled_primary_is_answered_by_the_stall_recompile() {
    use crate::compile::StallRule;
    use crate::output::AnsweredBy;
    use crate::test_support::clause;
    use std::time::{Duration, Instant};

    // The data format's example (`tests/data/mcc_mc_sat.cnf`): Arjun keeps
    // its reduction, so the count goes through the primary compile.
    let formula = crate::test_support::formula(
        6,
        vec![clause(&[-1, -2]), clause(&[2, 3, -4]), clause(&[4, 5]), clause(&[4, 6])],
    );
    let want = crate::test_support::brute_projected_count(&formula, 0..6);
    let eng = &tididi::Engine::new();
    let mc_config = || crate::config::Config {
        mode: crate::config::Mode::Mc,
        preprocess_level: crate::config::PreprocessLevel::Full,
        time_limit_ms: Some(3_600_000),
        ..crate::test_support::minimal_config()
    };
    let deadline = Some(Instant::now() + Duration::from_secs(3600));

    let config = mc_config();
    let run = crate::stage::Run { deadline, ..crate::test_support::run(eng, &config) };
    let rule = super::super::stall_rule(config.time_limit_ms);
    let plain = super::super::count_mc(&run, &formula, rule).expect("the count answers");
    assert_eq!((plain.count, plain.answered_by), (want.clone(), AnsweredBy::Compile));

    let config = mc_config();
    let run = crate::stage::Run { deadline, ..crate::test_support::run(eng, &config) };
    let forced = StallRule { wall: Duration::ZERO, min_left: 0 };
    let stalled = super::super::count_mc(&run, &formula, Some(forced)).expect("the stall recompile answers");
    assert_eq!((stalled.count, stalled.answered_by), (want, AnsweredBy::StallRecompile));
    let tree = run.started_on.borrow().clone().expect("the recompile named its tree");
    assert_eq!(crate::vtree::spec_family(&tree), "goatd-primal", "the run names {tree}");
}
