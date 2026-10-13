//! Vtree construction for the compiler, and the descriptors of a formula's
//! independent components.
//!
//! A vtree is a binary tree whose leaves are the formula's variables. The
//! diagram the compiler builds follows its shape, so the vtree decides how
//! large the diagram grows. Vitri builds every one, from a spec in its
//! [vtree grammar](https://github.com/Tractables/vitri/blob/main/docs/vtrees.md),
//! the one the binary's `--vtree` takes; `"portfolio"`, the default, builds a
//! catalog of candidates and keeps the best-scoring one.
//!
//! [`build_vtree`] builds one tree for the whole formula, and
//! [`build_vtree_with_components`] one per independent component as well when
//! the spec reads the formula's structure; [`VtreeOptions`] are the options of
//! one request. A component is a group of clauses that shares no variable with
//! the rest: [`Component`] names its clauses and variables, and
//! [`ComponentVtree`] pairs it with the tree built for it.
//!
//! Every construction asks vitri, and what it asks is decided here: the
//! selection context a request carries, the request itself and the conversion
//! of the trees that come back. The run's frontends (`crate::frontend`), which
//! have vitri build a vtree beside its preprocessing, use the same selection
//! context and conversion.

use std::sync::Arc;
use vitri::cnf::CnfFormula;
use tididi::vtree::{VarId, Vtree};

use crate::budget::timed_vtree_build;
use crate::config::Config;
use crate::convert::{diagram_var, vtree_from_vitri};
use crate::Error;

/// The options of one vtree request.
///
/// The default ranks the candidates of a portfolio spec by how evenly they
/// spread the clauses over the tree, with no deadline. Nothing about the
/// construction is read from ambient state apart from vitri's `VITRI_*`
/// vtree-selection settings, which every build reads from the environment.
#[derive(Clone, Debug)]
pub struct VtreeOptions {
    pub(crate) peak_active: bool,
    pub(crate) deadline: Option<std::time::Instant>,
    /// The portfolio's record of earlier builds. Cloned options share it, so
    /// the builds of one run's cascade see one another; a fresh constructor
    /// starts an independent history.
    pub(crate) portfolio_history: vitri::decompose::PortfolioBuildHistory,
    pub(crate) source_structure_profile: Option<vitri::score::StructureProfile>,
}

/// Clause-balance selection, no deadline, a fresh portfolio history.
impl Default for VtreeOptions {
    fn default() -> Self {
        Self {
            peak_active: false,
            deadline: None,
            portfolio_history: vitri::decompose::PortfolioBuildHistory::default(),
            source_structure_profile: None,
        }
    }
}

impl VtreeOptions {
    /// Selection that ranks the candidates by their peak context width over
    /// every variable; the projected counts ask for it when there is a show
    /// set.
    pub fn by_peak_width() -> Self {
        Self {
            peak_active: true,
            ..Self::default()
        }
    }

    /// Stop construction at `deadline`. `None` leaves it unbounded. A build
    /// that reaches it still returns a vtree: the candidate under way gets one
    /// short attempt and the rest are not started.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Option<std::time::Instant>) -> Self {
        self.deadline = deadline;
        self
    }

    #[must_use]
    pub(crate) fn with_portfolio_history(
        mut self,
        history: vitri::decompose::PortfolioBuildHistory,
    ) -> Self {
        self.portfolio_history = history;
        self
    }

    /// The run's portfolio history and the structure profile of its input.
    #[must_use]
    pub(crate) fn with_run_policy(mut self, config: &Config) -> Self {
        self.portfolio_history = config.portfolio_history.clone();
        self.source_structure_profile = config.source_structure_profile;
        self
    }
}

/// What a build returns: the vtree over every variable of the formula, and
/// the component trees when the build split the formula.
#[derive(Clone, Debug)]
pub struct VtreeBuild {
    /// The vtree over the formula's variables `1..=num_vars`.
    pub root: Arc<Vtree>,
    /// One tree per independent component, when the spec reads the formula's
    /// structure and the formula has two or more components; `None` when the
    /// build did not split. [`ComponentPlan::from_components`](crate::compile::ComponentPlan::from_components)
    /// turns it into the plan [`compile_cnf`](crate::compile::compile_cnf)
    /// takes.
    pub components: Option<Vec<ComponentVtree>>,
}

impl VtreeBuild {
    /// Attach freshly built trees to the run's engine before any diagram is
    /// built on them.
    pub(crate) fn bind(mut self, engine: &tididi::Engine) -> Self {
        self.root = engine.bind_vtree(Arc::unwrap_or_clone(self.root));
        self.components = self.components.map(|components| {
            components.into_iter().map(|mut component| {
                component.vtree = engine.bind_vtree(Arc::unwrap_or_clone(component.vtree));
                component
            }).collect()
        });
        self
    }
}

/// An independent component of a formula: its clauses, and the formula
/// variables it renumbers.
///
/// A component is compiled as a formula of its own over the local variables
/// `1..=k`, `k` being `local_to_global.len()`: local variable `i + 1` stands
/// for the formula variable `local_to_global[i]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    /// The component's clauses, as indices into the formula's clauses.
    pub clause_indices: Vec<usize>,
    /// The formula variable of each local variable, ascending: entry `i` is
    /// the one local variable `i + 1` stands for.
    pub local_to_global: Vec<VarId>,
}

/// A component and the vtree built for it, whose leaves are the component's
/// local variables.
#[derive(Clone, Debug)]
pub struct ComponentVtree {
    /// The vtree over the local variables `1..=component.local_to_global.len()`.
    pub vtree: Arc<Vtree>,
    /// The clauses and variables the vtree is for.
    pub component: Component,
}

/// Accept a spec of Vitri's vtree grammar, the one `--vtree` takes.
pub(crate) fn validate_vtree_spec(spec: &str) -> Result<(), Error> {
    vitri::spec::validate_vtree_spec(spec).map_err(|error| Error::VtreeSpec(error.to_string()))
}

/// The vitri request for one construction in the caller's window: the caller
/// has carved the absolute construction deadline out of its run, so vitri
/// spends all of what remains of it rather than dividing it again.
///
/// `spec` goes to Vitri as written; the caller times the build and supplies
/// every policy that has a Vitri representation.
fn vitri_vtree_build_config(
    spec: &str,
    deadline: Option<std::time::Instant>,
    components: vitri::config::ComponentPolicy,
) -> vitri::RunConfig {
    vitri::RunConfig {
        deadline,
        construction_budget: vitri::config::ConstructionBudget::WholeRemaining,
        vtree_spec: spec.to_owned(),
        components,
        candidates: 1,
        ..vitri::RunConfig::default()
    }
}

/// Translate TiDiDi's caller-owned selection context into Vitri's explicit one.
fn vitri_selection_ctx(ctx: &VtreeOptions) -> vitri::decompose::SelectionCtx {
    let mut selection = match ctx.peak_active {
        true => vitri::decompose::SelectionCtx::peak(),
        false => vitri::decompose::SelectionCtx::plain(),
    };
    selection.source_profile = ctx.source_structure_profile;
    selection.portfolio.build_history = ctx.portfolio_history.clone();
    selection
}

/// Resolve Vitri's settings for every compiler frontend.
pub(crate) fn resolve_vitri_selection_ctx(
    ctx: &VtreeOptions,
) -> Result<vitri::decompose::SelectionCtx, vitri::VitriError> {
    vitri_selection_ctx(ctx).with_env_defaults()
}

/// Consume one Vitri construction result at TiDiDi's type boundary.
///
/// Both standalone construction and the whole-run frontend use this exact
/// conversion, so component numbering and tree ownership cannot drift between
/// the two entry points.
pub(crate) fn vtree_build_from_vitri(built: vitri::component::VtreeBuild) -> VtreeBuild {
    let vitri::component::VtreeBuild {
        vtree,
        components,
        selections: _,
        candidate_sets: _,
        limits: _,
        cached_components: _,
        // The build's wall: a frontend bills it, and a standalone build is
        // timed by its caller instead.
        construction_ms: _,
    } = built;

    let components = components.map(|components| {
        components
            .into_iter()
            .map(|component| ComponentVtree {
                vtree: Arc::new(vtree_from_vitri(&component.vtree)),
                component: Component {
                    clause_indices: component.clause_indices,
                    local_to_global: component
                        .local_to_outer
                        .into_iter()
                        .map(diagram_var)
                        .collect(),
                },
            })
            .collect()
    });

    VtreeBuild { root: Arc::new(vtree_from_vitri(&vtree)), components }
}

/// Every standalone construction goes through here, so its selection context
/// and the wall it bills to [`crate::budget`] each happen once; a frontend
/// bills the wall vitri reports for its build instead. Vitri validates
/// `spec`.
fn build(
    formula: &CnfFormula,
    spec: &str,
    options: &VtreeOptions,
    components: vitri::config::ComponentPolicy,
) -> Result<VtreeBuild, Error> {
    let selection = resolve_vitri_selection_ctx(options)
        .map_err(|error| Error::Environment(error.to_string()))?;
    // A deadline that has already passed is Vitri's to answer, the same way it
    // answers one that passes mid-build: the first candidate gets one short
    // attempt and the rest are never started, so the build still returns a
    // vtree and the compile behind it decides against the wall.
    timed_vtree_build(|| {
        let config = vitri_vtree_build_config(spec, options.deadline, components);
        vitri::component::build_vtree(formula, &config, &selection)
            .map(vtree_build_from_vitri)
            .map_err(|error| Error::VtreeSpec(format!("vtree construction for {spec:?} failed: {error}")))
    })
}

/// Build one vtree for the whole formula from a spec, such as `"portfolio"`.
///
/// # Errors
///
/// - [`Error::VtreeSpec`]: `spec` is not a spec of vitri's grammar, or the
///   construction failed, as it does for a formula with no variables.
/// - [`Error::Environment`]: a `VITRI_*` vtree-selection setting in the
///   environment is invalid.
pub fn build_vtree(formula: &CnfFormula, spec: &str, options: &VtreeOptions) -> Result<Arc<Vtree>, Error> {
    Ok(build(formula, spec, options, vitri::config::ComponentPolicy::Whole)?.root)
}

/// Build a vtree for a formula from a spec, and one per independent component
/// when the spec reads the formula's structure and the formula has two or
/// more components.
///
/// [`VtreeOptions::default`] ranks a portfolio's candidates by clause balance;
/// [`VtreeOptions::by_peak_width`] by their peak context width over every
/// variable.
///
/// # Errors
///
/// As [`build_vtree`].
pub fn build_vtree_with_components(
    formula: &CnfFormula,
    spec: &str,
    options: &VtreeOptions,
) -> Result<VtreeBuild, Error> {
    build(formula, spec, options, vitri::config::ComponentPolicy::Split)
}

/// A stage's reading of a construction that failed: it says why on stderr
/// and gives up, as a compile that stops does, so the stage's own fallback
/// runs. A panic instead would reach the recovery around the compile, which
/// takes any panic for a refused allocation and splits the formula.
pub(crate) fn built_or_give_up<T>(built: Result<T, Error>) -> Option<T> {
    built.map_err(|error| eprintln!("[vtree] gave up: {error}")).ok()
}

/// Structural strategies read the primal/incidence graph and benefit from
/// per-component construction; the baselines build from the variable numbering
/// alone and do not.
///
/// The baselines are [`vitri::spec::baseline_spec_names`], matched by prefix
/// the way Vitri matches them, so `random-7` is the baseline `random` here too.
/// Vitri decides it with `VtreeBase::is_structural`, which is `pub(crate)`
/// there; this is the same rule spelled over the spec list Vitri exports.
fn is_structural_spec(spec: &str) -> bool {
    let base = spec.split(':').next().unwrap_or("");
    !vitri::spec::baseline_spec_names().any(|name| base.starts_with(name))
}

/// The components a streaming compile on `spec` takes `formula` in, as
/// [`build_vtree_with_components`] splits it.
///
/// Empty for a formula with no clauses; for a structural spec whose formula
/// splits into two or more independent components, one per component;
/// otherwise the whole formula as one.
pub(crate) fn split_components(formula: &CnfFormula, spec: &str) -> Vec<Component> {
    if formula.clauses().is_empty() {
        return vec![];
    }

    if is_structural_spec(spec)
        && let Some(groups) = formula.detect_components()
    {
        eprintln!("[vtree] {} independent components", groups.len());
        return groups
            .into_iter()
            .map(|clause_indices| Component {
                local_to_global: formula
                    .component_vars(&clause_indices)
                    .into_iter()
                    .map(diagram_var)
                    .collect(),
                clause_indices,
            })
            .collect();
    }

    vec![whole_formula_component(formula)]
}

/// The whole formula as one component, with the identity variable map.
pub(crate) fn whole_formula_component(formula: &CnfFormula) -> Component {
    Component {
        clause_indices: (0..formula.clauses().len()).collect(),
        local_to_global: (1..=formula.num_vars()).map(VarId).collect(),
    }
}

#[cfg(test)]
#[path = "tests/vtree.rs"]
mod tests;
