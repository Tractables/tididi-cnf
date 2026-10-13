//! The compiler: a CNF formula and a vtree in, a Tree Decision Diagram out.
//!
//! [`compile_cnf`] groups the clauses by their scope, the lowest vtree node
//! whose subtree covers every variable of the clause, and works bottom-up: at
//! each vtree node it conjoins the children's diagrams, then the clauses scoped
//! there, a batch at a time, minimizing as it goes. A formula whose clauses
//! fall into independent components, groups that share no variable, is
//! compiled one component at a time; [`ComponentPlan`] says whether the
//! compiler finds them, takes them from the vtree construction, or compiles
//! the formula whole. [`Output`] says what the compile produces: the Boolean
//! diagram, or a counting diagram that marginalizes each level once its last
//! clause is in, weighted or not, with variables forgotten for a projected
//! count, or the Boolean diagram of such a projection.
//!
//! A finished compile returns [`Compiled`]: the diagram, the largest size it
//! reached, and how many projected variables it left free. It never returns
//! a partial diagram: a compile that cannot finish says why with [`GiveUp`].
//!
//! The run [`count`](crate::count) counts with this same compile, one
//! component at a time where the formula splits, and retries a count that ran
//! out of memory on smaller pieces of the formula.

mod batch;
mod cofactor;
mod component;
mod marginalize;
mod minimize;
pub(crate) mod sat_prune;
mod schedule;
mod scope;
mod stall;
mod step;

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use num_bigint::BigUint;
use tididi::{Engine, OperationError};
use tididi::diagram::{Arithmetic, RationalWeights, Tdd, WeightStore, WeightValue};
use tididi::vtree::{VarId, Vtree};
use vitri::cnf::CnfFormula;

use crate::Error;
use crate::budget::{deadline_passed, deadline_scope};
use crate::problem::WeightedCount;
use crate::vtree::{Component, ComponentVtree};
use component::{CompileJob, PassOutput, compile_split, detected_split};
use minimize::minimize_full;
use step::compile_component;

pub(crate) use stall::{StallRule, StallWatch};

/// Which independent components a compile splits the formula into.
#[derive(Clone, Copy, Default)]
pub enum ComponentPlan<'a> {
    /// Find them; with two or more, compile each on a balanced vtree of its
    /// own and leave `vtree` unused.
    #[default]
    Detect,
    /// Compile the formula whole, against the supplied root vtree.
    Whole,
    /// Compile the components the caller already built trees for, in this order.
    Split(&'a [ComponentVtree]),
}

impl<'a> ComponentPlan<'a> {
    /// Interpret a construction's component trees: a component list is an
    /// exact split plan, and absence is an explicit whole-formula plan.
    pub fn from_components(components: Option<&'a [ComponentVtree]>) -> Self {
        match components {
            Some(components) => Self::Split(components),
            None => Self::Whole,
        }
    }
}

/// What a run sets for one compile, beyond what the compile is asked to
/// produce: the run's SAT pruning, whether the compile is a Shannon
/// cofactor's, and the per-step wall its internal steps run under.
#[derive(Clone, Copy, Default)]
pub(crate) struct CompileRun<'a> {
    /// The run's SAT pruning, which a compile that runs to its end adds its
    /// tallies to; `None` prunes nothing.
    pub(crate) sat_prune: Option<&'a sat_prune::SatPrune>,
    /// A recovery cofactor's compile, which skips the content-twin scan: the
    /// cascade recompiles cofactors under memory pressure, and scanning a
    /// diagram about to be split or dropped adds no useful work.
    pub(crate) cofactor: bool,
    /// The per-step wall the compile's internal steps run under, and where it
    /// reports a step that stopped on it ([`StallWatch`]); `None` for no
    /// per-step wall. Only the model count's primary compile has one.
    pub(crate) stall_watch: Option<&'a StallWatch>,
}

/// Why a compile has no diagram to return.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GiveUp {
    /// The compile stopped short at its deadline. A caller with a fallback
    /// runs it; nothing partial is ever returned, so a count taken from a
    /// compile that did not give up is exact. The library's
    /// `OperationError::Stopped` is the same stop, one level down.
    Stopped,
    /// The compile stopped short because an apply was refused under the batch
    /// limits: `limit` is `OverBudget` (memory) or `OutputCap` (size), and
    /// `refused_bytes` the request the allocator refused, when it was one. A
    /// caller treats it as [`GiveUp::Stopped`].
    Refused {
        /// The limit the apply reached.
        limit: OperationError,
        /// The size of the refused allocation, if the allocator refused one.
        refused_bytes: Option<u64>,
    },
    /// An allocation was refused while reducing the finished diagram. The
    /// binary reports it with the diagnostic [`crate::Error`] carries, which is
    /// what a benchmark harness reads to classify the run as out of memory.
    OutOfMemory,
    /// The caller's input does not fit together: the vtree does not place
    /// exactly the formula's variables, or a [`ComponentPlan::Split`] does not
    /// partition the formula. The text says which.
    Invalid(String),
}

impl GiveUp {
    /// The one reading of an `OperationError` a compile makes as it gives up:
    /// a refused apply names its limit and, when the allocator refused it,
    /// the size, which separates a request no machine could satisfy from a
    /// machine that is full. `Stopped` is the compile's own wall, or an inner
    /// budget's expiry riding the same unwind.
    ///
    /// # Panics
    ///
    /// On any other error: invalid operands are a compiler bug, not a stop.
    pub(crate) fn from_operation(eng: &Engine, e: OperationError) -> Self {
        match e {
            OperationError::OverBudget | OperationError::OutputCap => {
                GiveUp::Refused { limit: e, refused_bytes: eng.limits().meters().refused_reserve_bytes }
            }
            OperationError::Stopped => GiveUp::Stopped,
            other => panic!("compiler produced invalid operands: {other}"),
        }
    }

    /// Say on stderr why the compile stopped, for a refused apply: what a
    /// benchmark log is read for to tell a memory stop from a deadline. The
    /// other give-ups print nothing; their callers say what they do next.
    pub(crate) fn report(&self) {
        if let GiveUp::Refused { .. } = self {
            eprintln!("[compile] stopped: {self}");
        }
    }
}

/// A compile whose inputs the crate built, as its caller reads it: a give-up
/// is reported and becomes `None`, for the caller's fallback.
///
/// # Panics
///
/// On [`GiveUp::Invalid`]: the crate built inputs that do not fit together.
pub(crate) fn settle<T>(result: Result<T, GiveUp>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(GiveUp::Invalid(why)) => panic!("the crate built a compile input that does not fit: {why}"),
        Err(give_up) => {
            give_up.report();
            None
        }
    }
}

impl std::fmt::Display for GiveUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GiveUp::Stopped => f.write_str("the compile stopped at its deadline"),
            GiveUp::Refused { limit, refused_bytes: Some(b) } => write!(
                f,
                "the allocator refused {b} bytes ({:.1} GiB) during apply (cap={limit:?})",
                *b as f64 / (1u64 << 30) as f64,
            ),
            GiveUp::Refused { limit, refused_bytes: None } => {
                write!(f, "apply reached a limit with no recorded allocator refusal (cap={limit:?})")
            }
            GiveUp::OutOfMemory => f.write_str("an allocation was refused while reducing the compiled diagram"),
            GiveUp::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for GiveUp {}

/// A give-up as the run reports it: a stop or a refusal ends the run without
/// a count, and inputs that do not fit fail the compile stage.
impl From<GiveUp> for Error {
    fn from(give_up: GiveUp) -> Self {
        match give_up {
            GiveUp::Stopped | GiveUp::Refused { .. } => Error::compile_gave_up(),
            GiveUp::OutOfMemory => Error::out_of_memory(),
            GiveUp::Invalid(why) => Error::Stage(format!("compile failed: {why}")),
        }
    }
}

/// The heap around one [`release_failed_compile`], and how its reclamation
/// went: `Err` carries the first mallctl failure.
pub(crate) struct Released {
    pub(crate) before: crate::mem::HeapStats,
    pub(crate) after: crate::mem::HeapStats,
    pub(crate) reclaimed: Result<(), libc::c_int>,
}

/// Give back what a failed compile left behind, before the next compile
/// starts on the same engine.
///
/// The failed compile's diagrams are already dropped. What outlives them is
/// the engine's retained apply scratch (`Engine::clear_scratch`: the sparse
/// workspace and level pools the failure left pinned at full capacity), freed
/// first so the purge can advise its pages away, and the allocator's dirty
/// pages, which eager reclamation ([`crate::mem::reclaim_eagerly`]) returns.
/// Every compile that follows a failed one on the same engine runs this first:
/// the model count's fallbacks and Shannon split, and the plain compile's
/// retry on a second vtree.
pub(crate) fn release_failed_compile(eng: &Engine) -> Released {
    let before = crate::mem::HeapStats::read();
    eng.clear_scratch();
    let reclaimed = crate::mem::reclaim_eagerly();
    Released { before, after: crate::mem::HeapStats::read(), reclaimed }
}

/// What a compile produces.
#[derive(Clone, Default)]
pub enum Output<'a> {
    /// The formula's Boolean diagram.
    #[default]
    Function,
    /// A counting diagram: each level is marginalized as soon as the last
    /// clause mentioning its variables lands, so the diagram's `model_count` is
    /// the formula's and memory is freed as the compile climbs.
    Count,
    /// [`Output::Count`] with literal weights: the marginalization carries
    /// values in `arithmetic` instead of integers, and the result holds the
    /// store [`tididi::Engine::weighted_value`] reads the weighted count from.
    WeightedCount {
        /// The weight of each literal.
        weights: RationalWeights,
        /// The domain the values are computed in.
        arithmetic: Arithmetic,
    },
    /// [`Output::Count`] of the formula with these variables ∃-forgotten during
    /// the compile, each at its completion node, so the diagram never
    /// materializes the projected cone. A forgotten variable is left free and
    /// counts ×2, so a caller divides the count by `2^`[`Compiled::forgotten`].
    /// Every leaf level stays Boolean, so a later forget can read its label.
    ProjectedCount(&'a [VarId]),
    /// The Boolean diagram of the formula with these variables ∃-forgotten
    /// during the compile, as [`Output::ProjectedCount`] forgets them, each
    /// left free. No level is marginalized, so a weighted fold can read the
    /// whole result.
    ProjectedFunction(&'a [VarId]),
}

/// What a whole-formula compile is being asked for.
///
/// [`CompileOptions::default`] is the plain Boolean compile with component detection —
/// `compile_cnf(eng, &formula, &vtree, CompileOptions::default())`.
#[derive(Default)]
pub struct CompileOptions<'a> {
    /// Which components to compile separately, and against which vtrees.
    pub plan: ComponentPlan<'a>,
    /// What the compile produces. Variable ids it lists above
    /// `formula.num_vars` are ignored.
    pub output: Output<'a>,
    /// The wall-clock the compile gives up at. When set it is also installed
    /// as the apply-level cut for this compile, so one giant un-yielding
    /// conjoin can be stopped mid-apply rather than only at a step boundary.
    /// `None` leaves any enclosing apply deadline in force.
    pub deadline: Option<Instant>,
}

/// What a finished compile returns.
pub struct Compiled {
    /// The compiled diagram, on the vtree described at [`compile_cnf`].
    pub diagram: Tdd,
    /// How many of the variables a projected [`Output`] forgets are free in
    /// `diagram`, each doubling its count: those ∃-forgotten, and those in no
    /// clause, which a component split grafts in free or a clause-free formula
    /// leaves unconstrained. Zero for a compile that forgets nothing.
    pub forgotten: usize,
    /// The largest diagram, in pairs, any step of the compile reached; the
    /// largest over the components of a split compile, which run one after
    /// another.
    pub peak_pairs: usize,
}

impl Compiled {
    /// The count `diagram` holds onto the variables a projected [`Output`]
    /// keeps: its model count with the doubling of each
    /// [`forgotten`](Self::forgotten) variable divided out. The plain model
    /// count for a compile that forgets nothing.
    pub(crate) fn projected_count(&self, eng: &Engine) -> Result<BigUint, OperationError> {
        Ok(eng.model_count(&self.diagram)? >> self.forgotten)
    }
}

/// Compile a CNF formula into a canonical TDD, guided by the vtree.
///
/// By default ([`ComponentPlan::Detect`]) a formula whose clauses fall into
/// independent components, groups that share no variable, is compiled one
/// component at a time, each on a balanced vtree of its own, and the
/// components' diagrams are grafted into one; a formula with one component
/// is compiled on `vtree`. [`CompileOptions::plan`] can instead ask for the
/// whole formula on `vtree`, or for the caller's own component vtrees.
///
/// The result's vtree, the one its diagram carries, is the vtree to build any
/// further operand on: [`and`](tididi::and) and the other operations refuse
/// operands on different vtrees. It is `vtree` itself only when the formula was
/// compiled whole and no rotation fired. A split compile's result is on a vtree
/// grafted from the components' vtrees, and a counting compile may rotate its
/// vtree to bring marginal levels together, onto a copy; `vtree` itself is
/// never changed.
///
/// An [`Err`] is a give-up, never a partial diagram, so any count taken from an
/// `Ok` is exact. A caller that runs this under a deadline it set — an attempt
/// self-limiting at the wall, a reactive single shot with a fallback — treats
/// [`GiveUp::Stopped`] as its own loss.
///
/// An empty clause makes the result the false diagram, on `vtree`.
///
/// # Errors
///
/// - [`GiveUp::Stopped`]: [`CompileOptions::deadline`] passed.
/// - [`GiveUp::Refused`]: an apply was refused under the limits of the batch
///   `eng` belongs to.
/// - [`GiveUp::OutOfMemory`]: an allocation was refused while reducing the
///   finished diagram.
/// - [`GiveUp::Invalid`]: `vtree`, or a component's vtree, does not place
///   exactly its formula's variables, or a [`ComponentPlan::Split`] does not
///   partition the formula.
pub fn compile_cnf(
    eng: &Engine,
    formula: &CnfFormula,
    vtree: &Arc<Vtree>,
    opts: CompileOptions<'_>,
) -> Result<Compiled, GiveUp> {
    let sat_prune = sat_prune::SatPrune::default();
    compile_with(eng, formula, vtree, opts, CompileRun { sat_prune: Some(&sat_prune), ..CompileRun::default() })
}

/// [`compile_cnf`] under what the run sets for it (`run`).
pub(crate) fn compile_with(
    eng: &Engine,
    formula: &CnfFormula,
    vtree: &Arc<Vtree>,
    opts: CompileOptions<'_>,
    run: CompileRun<'_>,
) -> Result<Compiled, GiveUp> {
    let CompileOptions { plan, output, deadline } = opts;
    // The apply-level deadline, whose Drop restores the previous value on every
    // return and panic path.
    let _apply_deadline = deadline_scope(eng, deadline);

    let weights;
    let forget: HashSet<VarId>;
    let output = match output {
        Output::Function => PassOutput::Function,
        Output::Count => PassOutput::Count(None),
        Output::WeightedCount { weights: algebra, arithmetic } => {
            weights = WeightStore::new(algebra, arithmetic);
            PassOutput::Count(Some(&weights))
        }
        Output::ProjectedCount(vars) => {
            forget = formula_vars(formula, vars);
            PassOutput::ProjectedCount(&forget)
        }
        Output::ProjectedFunction(vars) => {
            forget = formula_vars(formula, vars);
            PassOutput::ProjectedFunction(&forget)
        }
    };
    let prune_spent_us = Cell::new(0);
    let job = CompileJob { deadline, output, run, prune_spent_us: &prune_spent_us };
    component::check_vtree(formula, vtree)?;

    if formula.clauses().is_empty() {
        let diagram = output.with_store(eng.one(vtree));
        // Every variable is free, ×2 in the raw count, a projected one included.
        let forgotten = output.forget().map_or(0, |forget| {
            (1..=vtree.num_vars()).filter(|&v| forget.contains(&VarId(v))).count()
        });
        return Ok(Compiled { diagram, forgotten, peak_pairs: 0 });
    }
    // The empty clause is the constant false, which the whole-formula pass
    // answers at once: there is nothing to split into components.
    let plan = if formula.clauses().iter().any(|c| c.literals.is_empty()) { ComponentPlan::Whole } else { plan };

    // Two compilation paths:
    //  1. A split — the caller's per-component vtrees, or detected components
    //     on balanced ones: compile each component on its own vtree (a
    //     marginal-cluster rotation stays confined to that vtree Arc), then
    //     *graft* the per-component TDDs into a single TDD on a freshly built
    //     grafted vtree.
    //  2. The whole formula (asked for, or a detected single component):
    //     compile on the caller's supplied root and minimize.
    match plan {
        ComponentPlan::Split(component_vtrees) => {
            return compile_split(eng, formula, component_vtrees, job);
        }
        ComponentPlan::Detect => {
            if let Some(detected) = formula.detect_components() {
                return compile_split(eng, formula, &detected_split(eng, formula, detected), job);
            }
        }
        ComponentPlan::Whole => {}
    }

    // Whole-formula compile.
    let mut result = compile_component(eng, formula, vtree, job)?;
    minimize_full(eng, &mut result.diagram, job.run.cofactor).map_err(|_| GiveUp::OutOfMemory)?;
    Ok(result)
}

/// The variables of `vars` that `formula` has: ids above `formula.num_vars`
/// are dropped.
fn formula_vars(formula: &CnfFormula, vars: &[VarId]) -> HashSet<VarId> {
    vars.iter().filter(|v| v.idx() < formula.num_vars() as usize).copied().collect()
}

/// The weighted count of `formula` under `algebra`, computed in
/// `arithmetic`: the [`Output::WeightedCount`] compile, read at its root. It
/// marginalizes as it climbs, so it frees memory as the model count does.
///
/// The verdict is the diagram's: a weighted compile stores no unsatisfiable
/// node, so the formula is unsatisfiable exactly when the result is the false
/// constant, whatever its weights.
///
/// # Returns
///
/// `None` when the compile gives up, at `deadline` or refused under the batch
/// limits (a refusal is reported on stderr), when the readout is refused, or
/// when the result has no weighted value.
pub(crate) fn weighted_count(
    eng: &Engine,
    formula: &CnfFormula,
    vtree: &Arc<Vtree>,
    components: ComponentPlan<'_>,
    algebra: RationalWeights,
    arithmetic: Arithmetic,
    deadline: Option<Instant>,
) -> Option<WeightedCount<WeightValue>> {
    let tdd = settle(compile_cnf(
        eng,
        formula,
        vtree,
        CompileOptions {
            plan: components,
            output: Output::WeightedCount { weights: algebra, arithmetic },
            deadline,
        },
    ))?
    .diagram;
    if tdd.is_zero() {
        return Some(WeightedCount::Unsatisfiable);
    }
    eng.weighted_value(&tdd).ok().flatten().map(WeightedCount::Satisfiable)
}

/// The model count of `formula`, one of `components` at a time: each
/// component's clauses are renumbered, `build_sub_vtree` builds its vtree,
/// and its count multiplies into the product before its diagram is dropped,
/// so the memory the count needs is the largest component's, not the sum.
/// Each variable in no component doubles the product.
///
/// `forget` makes it the count [`Output::ProjectedCount`] gives, onto the
/// other variables: each component's count is divided by its own forgotten
/// variables before it enters the product, so the result needs no correction.
///
/// `deadline` is installed as the apply-level cut, as [`compile_cnf`] does. It
/// does not bound `build_sub_vtree`, which runs no library operation. `run`
/// is what the run sets for each component's compile.
///
/// # Returns
///
/// The count and the largest diagram size any component's compile reached, in
/// pairs; or `None` when `build_sub_vtree` built no vtree for a component, or
/// a component's compile gave up, at `deadline` or refused under the batch
/// limits (a refusal is reported on stderr).
pub(crate) fn model_count_streaming<F>(
    eng: &Engine,
    formula: &CnfFormula,
    components: &[Component],
    mut build_sub_vtree: F,
    forget: Option<&[VarId]>,
    deadline: Option<Instant>,
    run: CompileRun<'_>,
) -> Option<(BigUint, usize)>
where
    F: FnMut(&CnfFormula) -> Option<Arc<Vtree>>,
{
    let forget = forget.map(|vars| formula_vars(formula, vars));

    if formula.clauses().is_empty() {
        return Some((
            BigUint::from(1u32) << component::streaming_free_var_count(formula, &[], forget.as_ref()),
            0,
        ));
    }

    let scratch = &mut component::RemapScratch::default();
    // Restored when the count returns, on every path.
    let _apply_deadline_guard = deadline_scope(eng, deadline);

    let prune_spent_us = Cell::new(0);
    let mut total = BigUint::from(1u32);
    let mut peak = 0usize;
    for comp in components {
        if deadline_passed(deadline) {
            return None;
        }
        component::remap_component_clauses_into(
            scratch,
            formula,
            &comp.clause_indices,
            &comp.local_to_global,
        );
        let sub_formula = scratch.sub();
        let sub_vtree = build_sub_vtree(sub_formula)?;
        // The projected variables, in this component's local numbering.
        let comp_forget =
            forget.as_ref().map(|forget| component::local_vars_in(&comp.local_to_global, forget));
        let (sub_count, sub_peak) = {
            let output = comp_forget.as_ref().map_or(PassOutput::Count(None), PassOutput::ProjectedCount);
            let job = CompileJob { deadline, output, run, prune_spent_us: &prune_spent_us };
            let compiled = settle(compile_component(eng, sub_formula, &sub_vtree, job))?;
            (compiled.projected_count(eng).ok()?, compiled.peak_pairs)
        };
        peak = peak.max(sub_peak);
        // A zero component zeroes the product: skip the rest of the loop.
        if sub_count == BigUint::ZERO {
            return Some((sub_count, peak));
        }
        total *= sub_count;
    }
    total <<= component::streaming_free_var_count(formula, components, forget.as_ref());
    Some((total, peak))
}

#[cfg(test)]
mod tests;
