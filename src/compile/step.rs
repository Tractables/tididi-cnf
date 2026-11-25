//! Bottom-up compile of one component: [`compile_component`], the traversal
//! every compile runs, on the whole formula or once per component, and the
//! per-vtree-node leaf and internal step functions it drives. It walks the
//! vtree bottom-up, hands each scope group's clauses to [`super::batch`] to
//! fold (or, at a node whose smaller child spans a few variables, to the
//! cofactor step in [`super::cofactor`]), applies the marginalize schedule
//! through [`super::marginalize`], and stops on the deadline.

use std::collections::HashSet;
use std::ops::ControlFlow;

use tididi::{Engine, Literal, OperationError};
use tididi::diagram::Tdd;
use tididi::vtree::{Home, VarId, VtreeIdx};
use vitri::cnf::CnfFormula;

use crate::budget::{deadline_passed, deadline_scope};
use crate::convert::diagram_literal;
use super::{Compiled, GiveUp};
use super::batch::batch_accumulate;
use super::cofactor::cofactor_step;
use super::component::CompileJob;
use super::marginalize::{build_marginalize_targets, run_marginalize_at};
use super::minimize::{ContractThrottle, MinimizeGate};
use super::schedule::VarSchedule;
use super::scope::{ScopeGroups, group_clauses_by_scope};

/// The marginal-clustering rotation's `bound_mult`: a trial's intermediate
/// pairs are limited to this multiple of the two levels' pair count (see
/// `Engine::rotate_marginal_cluster`).
const MARGINAL_CLUSTER_BOUND_FACTOR: usize = 32;

/// Read-only context every step of one bottom-up pass sees.
pub(crate) struct StepContext<'a> {
    /// The work order the compile was started with: its deadline and output.
    pub(crate) job: CompileJob<'a>,
    /// The formula's clauses in the diagram's literal type, converted once when
    /// the compile started.
    pub(crate) clauses: &'a DiagramClauses,
    /// Clauses grouped by vtree scope. `clauses_at[t.idx()]` is the list of
    /// clause indices whose scope is vtree node `t`.
    pub(crate) clauses_at: &'a [Vec<usize>],
    /// What each step may marginalize. `None` for an output that follows no
    /// schedule.
    pub(crate) marginalize_schedule: Option<&'a VarSchedule>,
}

/// What one bottom-up pass changes as it goes.
#[derive(Default)]
pub(crate) struct PassState {
    /// Per-vtree-node partial diagrams, indexed by `VtreeIdx::idx`.
    pub(crate) tdds: Vec<Option<Tdd>>,
    /// The adaptive-minimize gate.
    pub(crate) minimize: MinimizeGate,
    /// The contraction throttle of each diagram in `tdds`, by the same index.
    pub(crate) throttles: Vec<ContractThrottle>,
    /// The projected variables ∃-forgotten so far, in the pass's numbering.
    /// Each is forgotten once: at the step its leaf is scheduled at, or at the
    /// root.
    pub(crate) forgotten: HashSet<VarId>,
    /// The marginal-clustering rotation scratch (`Engine::rotate_marginal_cluster`).
    pub(crate) cluster_tried: Vec<u8>,
    /// The largest pair count any step's diagram reached during the pass.
    pub(crate) peak_size: usize,
}

impl PassState {
    fn new(total_nodes: usize, job: CompileJob<'_>) -> Self {
        let mut state = Self {
            tdds: vec![None; total_nodes],
            throttles: vec![ContractThrottle::default(); total_nodes],
            ..Self::default()
        };
        // A pass that sums nothing out has no marginal level, so no content
        // twins: skipping the scan spares the gate's per-fold test a sweep of
        // the levels.
        state.minimize.skip_content_twins = job.run.cofactor || !job.output.scheduled();
        state
    }
}

/// The formula's clauses in the diagram library's literal type.
///
/// vitri's literal and tididi's hold the same variable number and the same
/// polarity under different field names, so a clause converts by copying its
/// literals. That happens once, here, when a compile starts, rather than per
/// fold: the fold then hands the library a slice it takes as it is, and nothing
/// allocates while a clause is being conjoined.
pub(crate) struct DiagramClauses {
    literals: Vec<Literal>,
    /// `bounds[i]..bounds[i + 1]` is clause `i`'s span of `literals`.
    bounds: Vec<u32>,
}

impl DiagramClauses {
    pub(crate) fn new(formula: &CnfFormula) -> Self {
        let total = formula.clauses().iter().map(|c| c.literals.len()).sum();
        let mut clauses = Self::with_capacity(formula.clauses().len(), total);
        for clause in formula.clauses() {
            clauses.push(clause.literals.iter().map(|&lit| diagram_literal(lit)));
        }
        clauses
    }

    /// No clauses yet, with room for `clauses` of `literals` literals in all.
    pub(crate) fn with_capacity(clauses: usize, literals: usize) -> Self {
        let mut bounds = Vec::with_capacity(clauses + 1);
        bounds.push(0);
        DiagramClauses { literals: Vec::with_capacity(literals), bounds }
    }

    /// Append a clause, which takes the next index.
    pub(crate) fn push(&mut self, literals: impl IntoIterator<Item = Literal>) {
        self.literals.extend(literals);
        self.bounds.push(self.literals.len() as u32);
    }

    /// Clause `i`'s literals, indexed as they are in the formula.
    #[inline]
    pub(crate) fn get(&self, i: usize) -> &[Literal] {
        &self.literals[self.bounds[i] as usize..self.bounds[i + 1] as usize]
    }
}

/// Compile one component, all of `formula`, bottom-up against the vtree:
/// process each vtree node, conjoining child TDDs and accumulating local
/// clauses. The caller has checked that the vtree places the formula's
/// variables, or built it directly in that numbering.
///
/// [`GiveUp::Stopped`] and [`GiveUp::Refused`] are give-ups: the pass was cut
/// short by the deadline, or an apply was refused (`OverBudget` / `OutputCap`).
/// A node proved UNSAT answers ⊥ at once, however much is left uncompiled.
///
/// The run's [`VtreeTime`](crate::budget::VtreeTime) notes the formula and
/// vtree, and counts a compile that stopped at a deadline as budgeted work.
pub(crate) fn compile_component(
    eng: &Engine,
    formula: &CnfFormula,
    vtree: &Home,
    job: CompileJob<'_>,
) -> Result<Compiled, GiveUp> {
    let Some(work) = job.run.work else { return compile_pass(eng, formula, vtree, job) };
    work.note_compile(formula, vtree);
    work.budgeted_if(|| compile_pass(eng, formula, vtree, job), |compiled| matches!(compiled, Err(GiveUp::Stopped)))
}

fn compile_pass(
    eng: &Engine,
    formula: &CnfFormula,
    vtree: &Home,
    job: CompileJob<'_>,
) -> Result<Compiled, GiveUp> {
    // An empty clause makes the component the constant false, so it compiles
    // to ⊥ instead of reaching `clause_scope`'s assertion. Projected cutset
    // conditioning can produce one when preprocessing a conditioned UNSAT
    // branch.
    if crate::formula::has_empty_clause(formula) {
        return Ok(Compiled { diagram: eng.zero(vtree), forgotten: 0, peak_pairs: 0 });
    }

    // Decided before the pass starts: which clauses are conjoined at which
    // vtree node and in what order, and (for a scheduled output) when each
    // variable may be marginalized.
    let ScopeGroups { clauses_at, last_scope_pos } = group_clauses_by_scope(formula, vtree);
    let marginalize_schedule =
        job.output.scheduled().then(|| VarSchedule::new(vtree, &last_scope_pos, job.output.forget()));
    // Converted once for the bottom-up pass.
    let clauses = DiagramClauses::new(formula);

    // Start from state this attempt charges itself; the recovery cascade's
    // split compiles run on the same thread after it. Zero the in-flight byte
    // meter and the refused size a previous attempt left, so an allocator
    // refusal a give-up names is this attempt's own.
    eng.limits().reset_meters();
    let mut state = PassState::new(vtree.num_nodes(), job);
    let ctx = StepContext {
        job,
        clauses: &clauses,
        clauses_at: &clauses_at,
        marginalize_schedule: marginalize_schedule.as_ref(),
    };

    // Leaf levels: accumulate clauses local to each leaf (never has children).
    // Each step first tests the deadline; the deadline or an apply error aborts
    // the pass, and the caller decides what runs next.
    for (t, _var) in vtree.leaf_bottomup() {
        let step = match step_deadline(job) {
            deadline if deadline_passed(deadline) => Err(OperationError::Stopped),
            deadline => {
                let _cut = (deadline != job.deadline).then(|| deadline_scope(eng, deadline));
                fold_step(eng, t, &ctx, &mut state, vtree, None, None)
            }
        };
        if let ControlFlow::Break(out) = check_step(eng, vtree, step, &mut state, t) {
            return out;
        }
    }

    // Internal levels: conjoin child TDDs, then accumulate any clauses scoped
    // to this level.
    let topo: &[VtreeIdx] = vtree.internal_bottomup_slice();
    for &t in topo {
        let step = match step_deadline(job) {
            deadline if deadline_passed(deadline) => Err(OperationError::Stopped),
            deadline => {
                let _cut = (deadline != job.deadline).then(|| deadline_scope(eng, deadline));
                run_internal_step(eng, t, &ctx, &mut state, vtree)
            }
        };
        if let ControlFlow::Break(out) = check_step(eng, vtree, step, &mut state, t) {
            return out;
        }
        if let Err(e) = marginalize_and_rotate(eng, t, &ctx, &mut state, vtree) {
            return Err(GiveUp::from_operation(eng, e));
        }
    }

    let diagram = state.tdds[vtree.root().idx()].take().unwrap_or_else(|| eng.one(vtree));
    debug_assert!(diagram.vtree().is_on(vtree), "a pass returns its diagram on its home");
    Ok(Compiled { diagram, forgotten: state.forgotten.len(), peak_pairs: state.peak_size })
}

/// The deadline of the pass's next vtree node: the compile's own, or its
/// retry cut ([`super::CompileRun::retry_cut`]) once the allocator has gone
/// eager.
fn step_deadline(job: CompileJob<'_>) -> Option<std::time::Instant> {
    match job.run.retry_cut {
        Some(cut) if crate::mem::eager_decay_latched() => Some(job.deadline.map_or(cut, |deadline| deadline.min(cut))),
        _ => job.deadline,
    }
}

/// Run after each vtree node on its step's outcome and diagram: raise the
/// running peak to the pair count the step measured, keep that count on the
/// node's contraction throttle for its parent's lineage choice, and continue
/// while the diagram is satisfiable. UNSAT proves the formula UNSAT and answers ⊥ at
/// once; an error aborts the pass without making a satisfiability claim.
fn check_step(
    eng: &Engine,
    vtree: &Home,
    step: Result<usize, OperationError>,
    state: &mut PassState,
    t: VtreeIdx,
) -> ControlFlow<Result<Compiled, GiveUp>> {
    let step = step.inspect(|&pairs| {
        state.peak_size = state.peak_size.max(pairs);
        state.throttles[t.idx()].pairs = pairs;
    });
    let sat = step.and_then(|_| state.tdds[t.idx()].as_ref().map_or(Ok(true), |tdd| eng.is_sat(tdd)));
    match sat {
        Ok(true) => ControlFlow::Continue(()),
        Ok(false) => ControlFlow::Break(Ok(Compiled {
            diagram: eng.zero(vtree),
            forgotten: state.forgotten.len(),
            peak_pairs: state.peak_size,
        })),
        Err(error) => ControlFlow::Break(Err(GiveUp::from_operation(eng, error))),
    }
}

/// Scheduled marginalization and targeted marginal-clustering rotation after
/// internal step `t`: after step `t` marginalized levels in `subtree(t)`, try
/// rotations that bring two marginal levels under one parent so it too
/// collapses to counts, shrinking the in-flight diagram (see
/// `Engine::rotate_marginal_cluster`). The rotation changes the diagram's own
/// vtree, a projection of the home, and leaves the home as it is.
fn marginalize_and_rotate(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    vtree: &Home,
) -> Result<(), OperationError> {
    run_marginalize_at(eng, t, ctx, state, vtree)?;
    let PassState { tdds, cluster_tried, .. } = state;
    // A Boolean function has no marginal level to cluster.
    if let Some(tdd) = tdds[t.idx()].as_mut()
        && tdd.has_marginal_level()
    {
        cluster_tried.clear();
        let root = tdd.vtree().root();
        eng.rotate_marginal_cluster(tdd, root, MARGINAL_CLUSTER_BOUND_FACTOR, cluster_tried)?;
    }
    Ok(())
}

// ── The steps ────────────────────────────────────────────────────────────────

/// Fold node `t`'s clauses into `children` (the conjoined child diagrams, or
/// `None`), marginalizing in the fold, and store the result in `state.tdds[t]`.
/// The fold's contractions read and extend the throttle of node `lineage`'s
/// diagram, a fresh one when `None`, which then becomes `t`'s.
/// Returns its pair count, 0 when the node has no diagram. `Err(Stopped)`
/// when the deadline passes during the fold.
fn fold_step(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    vtree: &Home,
    children: Option<Tdd>,
    lineage: Option<VtreeIdx>,
) -> Result<usize, OperationError> {
    let targets = build_marginalize_targets(t, vtree, ctx, &state.forgotten);
    state.minimize.fold_at_root = t == vtree.root();
    let throttle = lineage.map(|c| std::mem::take(&mut state.throttles[c.idx()])).unwrap_or_default();
    state.minimize.swap_throttle(throttle);
    let folded = batch_accumulate(
        eng,
        &ctx.clauses_at[t.idx()], ctx, vtree, children,
        state, targets,
    );
    state.minimize.fold_at_root = false;
    state.throttles[t.idx()] = state.minimize.swap_throttle(ContractThrottle::default());
    state.tdds[t.idx()] = folded?;
    if deadline_passed(ctx.job.deadline) { return Err(OperationError::Stopped); }
    Ok(state.tdds[t.idx()].as_ref().map_or(0, Tdd::pair_count))
}

/// Process a single internal vtree node: the cofactor step when the node
/// qualifies for it ([`cofactor_step`]), otherwise conjoin the children and
/// fold the node's clauses ([`join_and_fold`]). Returns the pair count of
/// the node's diagram. `Err(Stopped)` when the deadline passes during the
/// step.
fn run_internal_step(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    vtree: &Home,
) -> Result<usize, OperationError> {
    match cofactor_step(eng, t, ctx, state, vtree)? {
        Some(pairs) => Ok(pairs),
        None => join_and_fold(eng, t, ctx, state, vtree),
    }
}

/// The normal internal step at `t`: conjoin the children's diagrams by
/// [`Engine::and`], fold the node's clauses into the result ([`fold_step`])
/// and store it in `state.tdds[t]`. Returns its pair count.
pub(super) fn join_and_fold(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    vtree: &Home,
) -> Result<usize, OperationError> {
    let (left, right) = vtree.children(t);
    // The larger child's diagram is the bulk of the fold's accumulator, so
    // its throttle is the one the fold carries on. The pair count its step
    // measured stands in for the diagram's: `pair_count` walks every level of
    // a diagram changed since it was last counted, as the marginalization
    // after a step changes it, which cost model counts on small instances 0.3%.
    let lineage = match (&state.tdds[left.idx()], &state.tdds[right.idx()]) {
        (Some(_), Some(_)) => {
            Some(if state.throttles[right.idx()].pairs > state.throttles[left.idx()].pairs { right } else { left })
        }
        (Some(_), None) => Some(left),
        (None, Some(_)) => Some(right),
        (None, None) => None,
    };
    let children_tdd = match (state.tdds[left.idx()].take(), state.tdds[right.idx()].take()) {
        (Some(l), Some(r)) => {
            if l.is_zero() || r.is_zero() {
                Some(eng.zero(vtree))
            } else {
                // On the projections under `t`'s two children, the library
                // joins them at `t` without a sweep.
                Some(eng.and(l, r)?)
            }
        }
        (Some(t_single), None) | (None, Some(t_single)) => Some(t_single),
        (None, None) => None,
    };
    fold_step(eng, t, ctx, state, vtree, children_tdd, lineage)
}

#[cfg(test)]
#[path = "tests/step.rs"]
mod tests;
