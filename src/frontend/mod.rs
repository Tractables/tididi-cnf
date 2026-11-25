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
#[derive(Clone)]
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
    pub(crate) vtree: Option<WmcVtree>,
    /// What each preprocessing stage did, and the wall it spent.
    pub(crate) report: Report,
}

/// The compile a frontend's tree is for. Only the run's main compile conjoins
/// vitri's implied clauses: a tree built after a compile gives up, such as a
/// Shannon cofactor's, gets none, so the recovery does not pay for the pass
/// again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TreeFor {
    MainCompile,
    Recovery,
}

/// Add to `built`, the tree a frontend built over `formula`, the clauses
/// vitri's implied-clause pass finds for it on the run's allowance
/// ([`request::implicates_ms`]), under `config`, the frontend's request. The
/// frontend itself runs no pass ([`frontend_config`]); this is where the
/// tree of a main compile gets its clauses. The pass counts as vtree time.
fn add_implied_clauses(
    built: &mut vitri::component::VtreeBuild,
    formula: &CnfFormula,
    parent: &Config,
    config: &vitri::RunConfig,
) {
    let config = vitri::RunConfig { implicates_ms: parent.vitri.implicates_ms, ..config.clone() };
    built.implied_clauses = parent.vtree_time.time(|| vitri::component::implied_clauses(formula, built, &config));
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
    // Only the fallback compiles on this tree, so the implied-clause pass
    // waits for it ([`WmcVtree::for_fallback`]).
    let config = frontend_config(parent, vitri::cnf::Mode::Wmc, deadline, false);
    let selection = run_selection(parent)?;
    let vitri::bundle::VitriRun { preprocessed, vtree, .. } =
        parent.vtree_time.count(|| vitri::bundle::run(formula, &meta, &config, &selection))?;
    let PreprocessBundle { reduced, record, stages, telemetry, .. } = preprocessed;
    let outcome = outcome::<Weighted>(reduced, &record)?;
    let vtree = match charged(vtree, &parent.vtree_time) {
        vitri::bundle::RunVtree::Built(built) => Some(WmcVtree(built)),
        vitri::bundle::RunVtree::FullyResolved | vitri::bundle::RunVtree::Refuted => None,
    };
    Ok(WmcFrontend { outcome, vtree, report: Report { stages, telemetry } })
}

/// The WMC frontend's vtree for the simplified formula, built with no
/// implied-clause pass.
pub(crate) struct WmcVtree(vitri::component::VtreeBuild);

impl WmcVtree {
    /// The tree for the fallback compile of `formula`, the simplified formula
    /// it was built for, with the clauses `formula` implies over it when the
    /// run asks for vitri's pass. The pass stops at `deadline` and its time
    /// counts as vtree time.
    pub(crate) fn for_fallback(
        self,
        formula: &CnfFormula,
        parent: &Config,
        deadline: Option<std::time::Instant>,
    ) -> VtreeBuild {
        let Self(mut built) = self;
        add_implied_clauses(&mut built, formula, parent, &frontend_config(parent, vitri::cnf::Mode::Wmc, deadline, false));
        vtree_build_from_vitri(built)
    }
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
    parent.vtree_time.budgeted(|| vitri::bundle::preprocess(formula, &vitri::cnf::CnfMeta::default(), &config))
}

/// Run vitri's paired function-preserving preprocess + vtree frontend and
/// translate its total original-variable map exactly once at this boundary.
/// The tree is the main compile's, with its implied clauses.
pub(crate) fn compile_frontend(
    formula: &CnfFormula,
    parent: &Config,
    deadline: Option<std::time::Instant>,
) -> Result<CompileFrontendAttempt, vitri::VitriError> {
    let meta = vitri::cnf::CnfMeta::default();
    let config = frontend_config(parent, vitri::cnf::Mode::Compile, deadline, true);
    let selection = run_selection(parent)?;
    let vitri::bundle::VitriRun { preprocessed, mut vtree, .. } =
        parent.vtree_time.count(|| vitri::bundle::run(formula, &meta, &config, &selection))?;
    let PreprocessBundle { reduced, record, telemetry, .. } = preprocessed;
    if let vitri::bundle::RunVtree::Built(built) = &mut vtree {
        add_implied_clauses(built, &reduced, parent, &config);
    }
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

/// A frontend's vtree in the compiler's type, its construction time charged
/// ([`charged`]).
fn frontend_vtree(vtree: vitri::bundle::RunVtree, vtree_time: &VtreeTime) -> FrontendVtree {
    match charged(vtree, vtree_time) {
        vitri::bundle::RunVtree::Built(built) => FrontendVtree::Built(vtree_build_from_vitri(built)),
        vitri::bundle::RunVtree::FullyResolved => FrontendVtree::FullyResolved,
        vitri::bundle::RunVtree::Refuted => FrontendVtree::Refuted,
    }
}

/// Every frontend's vtree crosses here, and the construction time vitri
/// reports for it is added to the run's `vtree_time` here: the frontend's one
/// call also preprocesses, so timing the call would count that too.
fn charged(vtree: vitri::bundle::RunVtree, vtree_time: &VtreeTime) -> vitri::bundle::RunVtree {
    if let vitri::bundle::RunVtree::Built(built) = &vtree {
        vtree_time.add(std::time::Duration::from_millis(built.construction_ms));
    }
    vtree
}

/// Run vitri's whole plain-MC frontend while keeping its borrowing session
/// scoped around the compiler continuation.
///
/// The input formula crosses into vitri once. The continuation gets the
/// prepared vitri session, so the cascade can ask it for the no-SBVA `retry`
/// without cloning or replaying the primary preprocessing work. The
/// continuation cannot let that borrowing session escape this function.
///
/// The prepared tree gets its implied clauses when it is for the main
/// compile (`tree`); a tree from the session's retry never does.
pub(crate) fn with_mc_frontend<R>(
    formula: &CnfFormula,
    parent: &Config,
    deadline: Option<std::time::Instant>,
    tree: TreeFor,
    continuation: impl FnOnce(McFrontendAttempt, &vitri::bundle::FrontendSession<'_>) -> R,
) -> Result<R, vitri::VitriError> {
    with_mc_frontend_adjusted(formula, parent, deadline, tree, |_| {}, continuation)
}

/// [`with_mc_frontend`] with `adjust` applied to the frontend's vitri
/// settings before it starts, for a fallback that asks vitri for a different
/// reduction or tree.
pub(crate) fn with_mc_frontend_adjusted<R>(
    formula: &CnfFormula,
    parent: &Config,
    deadline: Option<std::time::Instant>,
    tree: TreeFor,
    adjust: impl FnOnce(&mut vitri::RunConfig),
    continuation: impl FnOnce(McFrontendAttempt, &vitri::bundle::FrontendSession<'_>) -> R,
) -> Result<R, vitri::VitriError> {
    let meta = vitri::cnf::CnfMeta::default();
    let mut config = frontend_config(parent, vitri::cnf::Mode::Mc, deadline, true);
    adjust(&mut config);
    let selection = run_selection(parent)?;
    let mut session = parent.vtree_time.budgeted(|| vitri::bundle::frontend(formula, &meta, &config, &selection))?;
    let mut run = parent.vtree_time.count(|| session.prepare())?;
    if let (TreeFor::MainCompile, vitri::bundle::RunVtree::Built(built)) = (tree, &mut run.vtree) {
        add_implied_clauses(built, &run.preprocessed.reduced, parent, &config);
    }
    let attempt = mc_frontend_attempt(run, &parent.vtree_time);
    Ok(continuation(attempt, &session))
}

#[cfg(test)]
mod tests;
