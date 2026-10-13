//! The vitri requests this compiler builds.
//!
//! The `VITRI_*` controls in `vitri::RunConfig` are read when a run starts, by
//! [`resolve_env_config`], into the run's `Config::vitri`; the vtree-selection
//! controls are read on each build, by
//! [`resolve_vitri_selection_ctx`],
//! and checked when the run starts. A run's frontend ([`frontend_config`])
//! starts from the run's settings and sets only what the compiler owns: its
//! wall, its simplify level, whether Arjun runs and the vtree spelling. A stage
//! a runner asks for later ([`arjun_stage`], [`projection_stage`]) starts from
//! vitri's defaults with the run's Arjun options and the allowance the runner
//! computed. Every vitri knob this compiler sets is set in one of the two.

use crate::Error;
use crate::config::{Config, PreprocessLevel};
use crate::vtree::{VtreeOptions, resolve_vitri_selection_ctx};

/// Read and check vitri's environment settings for the run starting now.
///
/// Both the binary and `count` call this through the shared driver before
/// any work, so every run reads the environment as it is then. Construction
/// controls are checked here and read by each build; the `RunConfig`
/// preprocessing controls it returns go into the run's `Config`, so every
/// request built below starts from the same validated settings.
///
/// # Errors
///
/// [`Error`] when a `VITRI_*` setting does not parse, when it asks vitri to
/// export Arjun's learnt clauses, which this compiler does not use, or when
/// `VITRI_BUDGET_MS` is set at all: every request below sets vitri's budget
/// from the run's own time limit, so the variable would do nothing.
pub(crate) fn resolve_env_config() -> Result<vitri::RunConfig, Error> {
    let invalid = |error: vitri::VitriError| Error::Environment(error.to_string());
    resolve_vitri_selection_ctx(&VtreeOptions::default()).map_err(invalid)?;
    if std::env::var_os("VITRI_BUDGET_MS").is_some() {
        return Err(Error::Environment(
            "VITRI_BUDGET_MS is set, but a run sets vitri's budget from its own \
             time limit, so the variable would do nothing; unset VITRI_BUDGET_MS and \
             give the run a time limit instead."
                .to_owned(),
        ));
    }
    let config = vitri::RunConfig::from_env_defaults().map_err(invalid)?;
    if config.arjun.export_learned_clauses {
        return Err(Error::Environment(
            "VITRI_ARJUN_EXPORT_LEARNED_CLAUSES is set, but this build does not \
             use Arjun's learnt clauses; unset it."
                .to_owned(),
        ));
    }
    Ok(config)
}

/// The run's frontend: the run's vitri settings under its wall,
/// the parent's simplify level, and a portfolio vtree paired with the reduced
/// formula. `arjun` keeps vitri's Arjun stage as the run's vitri settings have
/// it (retaining its input checkpoint for a counting run, whose cascade may
/// retry from it) or switches it off. Function compilation asks for the parent's own
/// vtree spelling, which vitri validates when the run starts; every counting
/// mode asks for the portfolio.
pub(crate) fn frontend_config(
    parent: &Config,
    mode: vitri::cnf::Mode,
    deadline: Option<std::time::Instant>,
    arjun: bool,
) -> vitri::RunConfig {
    let mut config = parent.vitri.clone();
    config.budget_ms = parent.time_limit_ms;
    config.deadline = deadline;
    config.mode = Some(mode);
    config.stages.simplify = parent.preprocess_level == PreprocessLevel::Full;
    if mode == vitri::cnf::Mode::Compile {
        config.vtree_spec = parent.vtree_spec().to_owned();
    } else {
        config.vtree_spec = "portfolio".to_owned();
    }
    if arjun {
        config.retain_arjun_input = mode != vitri::cnf::Mode::Compile;
    } else {
        config.stages.arjun = false;
    }
    config
}

/// Arjun under an exact allowance and nothing else, on a formula a runner
/// already holds, keeping a result whose clause count grew as `growth`
/// allows.
pub(crate) fn arjun_stage(
    parent: &Config,
    mode: vitri::cnf::Mode,
    deadline: Option<std::time::Instant>,
    budget: std::time::Duration,
    growth: vitri::config::ArjunClauseGrowth,
) -> vitri::RunConfig {
    stage_config(parent, mode, deadline, Some(budget), growth, false, vitri::config::ProjectionPolicy::default())
}

/// The projection chain under `policy`, with Arjun under an exact allowance
/// when one is given and off otherwise (the chain's tail).
pub(crate) fn projection_stage(
    parent: &Config,
    mode: vitri::cnf::Mode,
    deadline: Option<std::time::Instant>,
    arjun: Option<std::time::Duration>,
    policy: vitri::config::ProjectionPolicy,
) -> vitri::RunConfig {
    stage_config(parent, mode, deadline, arjun, vitri::config::ArjunClauseGrowth::default(), true, policy)
}

/// The vitri request for one stage: Arjun under the exact allowance `arjun`
/// (off when `None`) and the clause-growth policy `growth`, with `budget_ms`
/// unset, so vitri does not divide the allowance again. The Arjun options are
/// the run's, on every mode.
fn stage_config(
    parent: &Config,
    mode: vitri::cnf::Mode,
    deadline: Option<std::time::Instant>,
    arjun: Option<std::time::Duration>,
    growth: vitri::config::ArjunClauseGrowth,
    simplify: bool,
    projection: vitri::config::ProjectionPolicy,
) -> vitri::RunConfig {
    vitri::RunConfig {
        budget_ms: None,
        deadline,
        arjun_budget: arjun.map(vitri::config::ArjunBudget::Exact).unwrap_or_default(),
        arjun_clause_growth: growth,
        projection_policy: projection,
        stages: vitri::config::PreprocessStages {
            simplify,
            arjun: arjun.is_some(),
        },
        mode: Some(mode),
        arjun: parent.vitri.arjun,
        ..vitri::RunConfig::default()
    }
}

#[cfg(test)]
#[path = "tests/request.rs"]
mod tests;
