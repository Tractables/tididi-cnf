//! The run's calls into vitri's preprocessing, the requests they make and
//! the records they return.
//!
//! The entry points here run a frontend: vitri's preprocessing and vtree
//! construction as one call, so the reduced formula and the tree built for it
//! cross the boundary together, and each frontend hands its driver a single
//! value; or the preprocessing alone, for the model count under an explicit
//! `--vtree`. vitri owns the pairing: every tree it builds has one leaf per
//! reduced variable, and it builds none for a refuted or fully resolved
//! record, so nothing here re-checks it. [`request`] builds every vitri
//! request from the run's configuration, [`record`] reads what a
//! preprocessing call returns, and [`expand`] puts a compiled diagram back
//! into the variables the record maps.

pub(crate) mod expand;
pub(crate) mod record;
pub(crate) mod request;

use vitri::bundle::PreprocessBundle;
use vitri::cnf::CnfFormula;

use crate::budget::VtreeTime;
use crate::config::Config;
use crate::problem::Weighted;
use crate::vtree::{VtreeBuild, VtreeOptions, resolve_vitri_selection_ctx, vtree_build_from_vitri};
use record::{Outcome, Report, original_expansion, outcome};
use request::frontend_config;

/// One plain-model-counting frontend attempt: vitri's preprocessing result as
/// it returned it, and the vtree built for its reduced formula, in tididi's
/// type.
///
/// The compiler may retain the accompanying
/// [`vitri::bundle::FrontendSession`] only through the scoped continuation
/// below, when it needs to request vitri's checkpointed retry.
pub(crate) struct McFrontendAttempt {
    pub(crate) preprocessed: PreprocessBundle,
    pub(crate) vtree: FrontendVtree,
}

/// The vtree a frontend attempt built, or why it built none.
pub(crate) enum FrontendVtree {
    Built(VtreeBuild),
    /// Preprocessing assigned every variable; the answer is the lift alone.
    FullyResolved,
    /// Preprocessing refuted the instance: the count is 0 and the compiled
    /// function is constant false. Kept apart from `FullyResolved`, whose
    /// answer is the lift: a refuted attempt still carries the original
    /// variable count, so reading it as a full resolution would emit the lift
    /// instead of zero.
    Refuted,
}

/// One function-preserving frontend result from vitri: the reduced formula, the
/// expansion plan that maps it back to the original variables, and the vtree
/// built alongside it.
pub(crate) struct CompileFrontendAttempt {
    pub(crate) reduced: CnfFormula,
    pub(crate) expansion: tididi::restructure::VariableExpansion,
    pub(crate) telemetry: vitri::bundle::PreprocessTelemetry,
    pub(crate) vtree: FrontendVtree,
}

/// One WMC frontend result: formula, weights, lift and vtree cross together.
pub(crate) struct WmcFrontend {
    /// The problem as the simplify left it, or its refutation.
    pub(crate) outcome: Outcome<Weighted>,
    /// The vtree built for the simplified formula; `None` when nothing is
    /// left to compile, because the record is UNSAT or fully resolved.
    pub(crate) vtree: Option<VtreeBuild>,
    /// What each preprocessing stage did, and the wall it spent.
    pub(crate) report: Report,
}

/// The vtree selection every frontend of a run makes: the run's portfolio
/// history and input profile, with vitri's `VITRI_*` settings.
fn run_selection(parent: &Config) -> Result<vitri::decompose::SelectionCtx, vitri::VitriError> {
    resolve_vitri_selection_ctx(&VtreeOptions::default().with_run_policy(parent))
}

/// Run vitri's paired WMC simplify + portfolio-vtree frontend. Arjun is
/// deliberately absent; the cascade owns its separately budgeted Arjun stage.
pub(crate) fn wmc_frontend(
    formula: &CnfFormula,
    weights: &vitri::cnf::WeightTable,
    parent: &Config,
    deadline: Option<std::time::Instant>,
) -> Result<WmcFrontend, vitri::VitriError> {
    let meta = vitri::cnf::CnfMeta::from_parts(
        formula.num_vars(),
        Some(vitri::cnf::Mode::Wmc),
        None,
        Some(weights.clone()),
    )?;
    let config = frontend_config(parent, vitri::cnf::Mode::Wmc, deadline, false);
    let vitri::bundle::VitriRun { preprocessed, vtree, .. } =
        vitri::bundle::run(formula, &meta, &config, &run_selection(parent)?)?;
    let PreprocessBundle { reduced, record, stages, telemetry, .. } = preprocessed;
    let outcome = outcome::<Weighted>(reduced, &record)?;
    let vtree = match frontend_vtree(vtree, &parent.vtree_time) {
        FrontendVtree::Built(built) => Some(built),
        FrontendVtree::FullyResolved | FrontendVtree::Refuted => None,
    };
    Ok(WmcFrontend { outcome, vtree, report: Report { stages, telemetry } })
}

/// Simplify-only preprocessing for the streaming model count (an explicit
/// `--vtree`). Arjun is not requested, so the reduced formula and the
/// simplify lift it returns are all the preprocessing the compile gets.
pub(crate) fn mc_streaming_preprocess(
    formula: &CnfFormula,
    parent: &Config,
    deadline: Option<std::time::Instant>,
) -> Result<PreprocessBundle, vitri::VitriError> {
    let config = frontend_config(parent, vitri::cnf::Mode::Mc, deadline, false);
    vitri::bundle::preprocess(formula, &vitri::cnf::CnfMeta::default(), &config)
}

/// Run vitri's paired function-preserving preprocess + vtree frontend and
/// translate its total original-variable map exactly once at this boundary.
pub(crate) fn compile_frontend(
    formula: &CnfFormula,
    parent: &Config,
    deadline: Option<std::time::Instant>,
) -> Result<CompileFrontendAttempt, vitri::VitriError> {
    let meta = vitri::cnf::CnfMeta::default();
    let config = frontend_config(parent, vitri::cnf::Mode::Compile, deadline, true);
    let vitri::bundle::VitriRun { preprocessed, vtree, .. } =
        vitri::bundle::run(formula, &meta, &config, &run_selection(parent)?)?;
    let PreprocessBundle { reduced, record, telemetry, .. } = preprocessed;
    let expansion = original_expansion(&record, reduced.num_vars());
    Ok(CompileFrontendAttempt {
        reduced,
        expansion,
        telemetry,
        vtree: frontend_vtree(vtree, &parent.vtree_time),
    })
}

/// Convert one plain-MC frontend run, the first or a later attempt from a live
/// session, into the compiler's types, converting the vtree once and adding
/// its construction to the run's `vtree_time`.
pub(crate) fn mc_frontend_attempt(run: vitri::bundle::VitriRun, vtree_time: &VtreeTime) -> McFrontendAttempt {
    McFrontendAttempt { preprocessed: run.preprocessed, vtree: frontend_vtree(run.vtree, vtree_time) }
}

/// Every frontend's vtree crosses here, and the construction time vitri
/// reports for it is added to the run's `vtree_time` here: the frontend's one
/// call also preprocesses, so timing the call would count that too.
fn frontend_vtree(vtree: vitri::bundle::RunVtree, vtree_time: &VtreeTime) -> FrontendVtree {
    match vtree {
        vitri::bundle::RunVtree::Built(built) => {
            vtree_time.add(std::time::Duration::from_millis(built.construction_ms));
            FrontendVtree::Built(vtree_build_from_vitri(built))
        }
        vitri::bundle::RunVtree::FullyResolved => FrontendVtree::FullyResolved,
        vitri::bundle::RunVtree::Refuted => FrontendVtree::Refuted,
    }
}

/// Run vitri's whole plain-MC frontend while keeping its borrowing session
/// scoped around the compiler continuation.
///
/// The input formula crosses into vitri once. The continuation gets the
/// prepared vitri session, so the cascade can ask it for the no-SBVA `retry`
/// without cloning or replaying the primary preprocessing work. The
/// continuation cannot let that borrowing session escape this function.
pub(crate) fn with_mc_frontend<R>(
    formula: &CnfFormula,
    parent: &Config,
    deadline: Option<std::time::Instant>,
    continuation: impl FnOnce(McFrontendAttempt, &vitri::bundle::FrontendSession<'_>) -> R,
) -> Result<R, vitri::VitriError> {
    let meta = vitri::cnf::CnfMeta::default();
    let config = frontend_config(parent, vitri::cnf::Mode::Mc, deadline, true);
    let mut session = vitri::bundle::frontend(formula, &meta, &config, &run_selection(parent)?)?;
    let run = session.prepare()?;
    let attempt = mc_frontend_attempt(run, &parent.vtree_time);
    Ok(continuation(attempt, &session))
}

#[cfg(test)]
mod tests;
