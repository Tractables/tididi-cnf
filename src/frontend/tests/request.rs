use super::*;
use vitri::cnf::Mode;

fn parent() -> crate::config::Config {
    crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::Full,
        time_limit_ms: Some(45_000),
        ..crate::test_support::minimal_config()
    }
}

/// vitri's default simplify policy with the equivalence-probing budget the
/// frontend sets.
fn probing_for(equivalence_budget_ms: u64) -> vitri::config::SimplifyPolicy {
    vitri::config::SimplifyPolicy {
        equivalence_budget_ms: Some(equivalence_budget_ms),
        ..vitri::config::SimplifyPolicy::default()
    }
}

#[test]
fn frontend_maps_the_parent_wall_and_simplify_level() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let parent = parent();
    let config = frontend_config(&parent, Mode::Mc, Some(deadline), true);

    assert_eq!(config.budget_ms, Some(45_000));
    assert_eq!(config.deadline, Some(deadline));
    assert_eq!(config.mode, Some(Mode::Mc));
    assert_eq!(config.vtree_spec, "portfolio");
    assert!(config.retain_arjun_input);
    assert!(config.stages.simplify);
    assert_eq!(config.simplify, probing_for(11_250));
    assert_eq!(config.arjun, parent.vitri.arjun);
    assert_eq!(config.arjun_budget, parent.vitri.arjun_budget);
    assert_eq!(config.stages.arjun, parent.vitri.stages.arjun);
    config.validate().expect("the MC frontend is valid");
}

#[test]
fn frontend_without_arjun_disables_the_stage_and_its_outputs_only() {
    let ordinary = frontend_config(&parent(), Mode::Mc, None, true);
    let streaming = frontend_config(&parent(), Mode::Mc, None, false);

    assert!(!streaming.stages.arjun);
    assert!(!streaming.retain_arjun_input);
    assert_eq!(streaming.stages.simplify, ordinary.stages.simplify);
    assert_eq!(streaming.simplify, ordinary.simplify);
    assert_eq!(streaming.deadline, ordinary.deadline);
    assert_eq!(streaming.budget_ms, ordinary.budget_ms);
    streaming.validate().expect("the simplify-only MC configuration is valid");

    let weighted = frontend_config(&parent(), Mode::Wmc, None, false);
    assert_eq!(weighted.mode, Some(Mode::Wmc));
    assert!(!weighted.stages.arjun);
    assert!(!weighted.retain_arjun_input);
    weighted.validate().expect("the weighted frontend config is valid");
}

#[test]
fn frontend_with_simplify_off_leaves_no_inert_knob() {
    let parent = crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::None,
        ..crate::test_support::minimal_config()
    };
    let config = frontend_config(&parent, Mode::Mc, None, true);
    assert!(!config.stages.simplify);
    config.validate().expect("the disabled simplify policy must not leave inert knobs");
}

#[test]
fn frontend_for_compilation_uses_the_parents_vtree_spec() {
    let parent = crate::config::Config {
        vtree: Some("minfill-primal".to_owned()),
        ..parent()
    };
    let config = frontend_config(&parent, Mode::Compile, None, true);
    assert_eq!(config.mode, Some(Mode::Compile));
    assert_eq!(config.vtree_spec, "minfill-primal");
    assert!(!config.retain_arjun_input);
    assert_eq!(config.simplify, probing_for(11_250));
    config.validate().expect("the compile config is valid");
}

#[test]
fn equivalence_probing_gets_a_quarter_of_the_limit_up_to_its_cap() {
    let cases = [(Some(120_000), 30_000), (Some(40_000), 10_000), (Some(3_600_000), 30_000), (None, 30_000)];
    for (time_limit_ms, expected) in cases {
        assert_eq!(equivalence_budget_ms(time_limit_ms), expected, "{time_limit_ms:?}");
        let parent = crate::config::Config { time_limit_ms, ..parent() };
        // The arguments each frontend in `frontend` passes.
        let frontends = [(Mode::Compile, true), (Mode::Mc, true), (Mode::Mc, false), (Mode::Wmc, false)];
        for (mode, arjun) in frontends {
            let config = frontend_config(&parent, mode, None, arjun);
            assert_eq!(config.simplify, probing_for(expected), "{mode:?} under {time_limit_ms:?}");
            config.validate().expect("the widened probing budget is a valid request");
        }
    }

    let simplify_off = crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::None,
        time_limit_ms: Some(120_000),
        ..parent()
    };
    let config = frontend_config(&simplify_off, Mode::Mc, None, true);
    assert_eq!(config.simplify, vitri::config::SimplifyPolicy::default());
    config.validate().expect("simplify off leaves the default policy");
}

#[test]
fn weighted_arjun_stage_uses_the_exact_budget_and_the_runs_options() {
    let budget = std::time::Duration::from_millis(1_234);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let config = arjun_stage(&parent(), Mode::Wmc, Some(deadline), budget, vitri::config::ArjunClauseGrowth::RejectAgainst(17));

    assert_eq!(config.deadline, Some(deadline));
    assert_eq!(config.budget_ms, None, "vitri must not divide the allowance again");
    assert_eq!(config.arjun_budget, vitri::config::ArjunBudget::Exact(budget));
    assert_eq!(
        config.arjun_clause_growth,
        vitri::config::ArjunClauseGrowth::RejectAgainst(17),
    );
    assert_eq!(config.mode, Some(Mode::Wmc));
    assert_eq!(
        config.stages,
        vitri::config::PreprocessStages { simplify: false, arjun: true },
    );
    assert_eq!(config.arjun, parent().vitri.arjun);
}

#[test]
fn projected_stages_keep_simplify() {
    let budget = std::time::Duration::from_millis(1_234);
    let arjun = projection_stage(
        &parent(),
        Mode::Pmc,
        None,
        Some(budget),
        vitri::config::ProjectionPolicy::ArjunOnly(vitri::config::ProjectionNoGain::Reject),
    );
    assert!(arjun.stages.arjun && arjun.stages.simplify);
    assert_eq!(arjun.arjun_budget, vitri::config::ArjunBudget::Exact(budget));

    let tail = projection_stage(&parent(), Mode::Pmc, None, None, vitri::config::ProjectionPolicy::Full);
    assert!(!tail.stages.arjun);
    assert_eq!(tail.arjun_budget, vitri::config::ArjunBudget::Derived);
    assert_eq!(tail.projection_policy, vitri::config::ProjectionPolicy::Full);
}
