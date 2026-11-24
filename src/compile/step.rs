//! Bottom-up compile of one component: [`compile_component`], the traversal
//! every compile runs, on the whole formula or once per component, and the
//! per-vtree-node leaf and internal step functions it drives. It walks the
//! vtree bottom-up, hands each scope group's clauses to [`super::batch`] to
//! fold (or, at a node whose smaller child spans a few variables, to the
//! cofactor step in [`super::cofactor`]), applies the marginalize schedule
//! through [`super::marginalize`], and stops on the deadline.

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::sync::Arc;

use tididi::{Engine, Literal, OperationError};
use tididi::diagram::Tdd;
use tididi::vtree::{VarId, Vtree, VtreeIdx};
use vitri::cnf::CnfFormula;

use crate::budget::{deadline_passed, deadline_scope};
use crate::convert::diagram_literal;
use super::{Compiled, GiveUp};
use super::batch::batch_accumulate;
use super::cofactor::cofactor_step;
use super::component::CompileJob;
use super::marginalize::{Schedule, build_marginalize_targets, run_marginalize_at};
use super::minimize::{ContractThrottle, MinimizeGate};
use super::sat_prune::{PruneOutcome, SatPruneCtx};
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
    pub(crate) marginalize_schedule: Option<&'a Schedule>,
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
    /// The SAT pruning context; `None` when the compile does not prune.
    pub(crate) sat_prune: Option<SatPruneCtx>,
    /// The batch builder's spine-marking scratch, one flag per vtree node,
    /// grown on first use and all false between batches, so the zeroing is
    /// paid once per pass rather than once per batch.
    pub(crate) on_spine: Vec<bool>,
    /// The marginal-clustering rotation scratch (`Engine::rotate_marginal_cluster`).
    pub(crate) cluster_tried: Vec<u8>,
    /// The largest pair count any step's diagram reached during the pass.
    pub(crate) peak_size: usize,
}

impl PassState {
    fn new(formula: &CnfFormula, vtree: &Arc<Vtree>, total_nodes: usize, job: CompileJob<'_>) -> Self {
        // A weighted compile builds no pruning context: its marginal levels hold
        // weights, and the per-node counts a pass orders its probes by are
        // refused for them.
        let weighted = vtree.home().is_some_and(|home| home.weights().is_some());
        let sat_prune = match job.run.sat_prune {
            Some(run_pass) if !weighted => {
                run_pass.context(formula, job.run.cofactor, job.prune_spent_us.get())
            }
            _ => None,
        };
        let mut state = Self {
            tdds: vec![None; total_nodes],
            throttles: vec![ContractThrottle::default(); total_nodes],
            sat_prune,
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
    vtree: &Arc<Vtree>,
    job: CompileJob<'_>,
) -> Result<Compiled, GiveUp> {
    let Some(work) = job.run.work else { return compile_pass(eng, formula, vtree, job) };
    work.note_compile(formula, vtree);
    work.budgeted_if(|| compile_pass(eng, formula, vtree, job), |compiled| matches!(compiled, Err(GiveUp::Stopped)))
}

fn compile_pass(
    eng: &Engine,
    formula: &CnfFormula,
    vtree: &Arc<Vtree>,
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
        job.output.scheduled().then(|| Schedule::new(vtree, &last_scope_pos, job.output, job.run.projections));
    // Converted once for the bottom-up pass.
    let clauses = DiagramClauses::new(formula);

    // Start from state this attempt charges itself; the recovery cascade's
    // split compiles run on the same thread after it. Zero the in-flight byte
    // meter and the refused size a previous attempt left, so an allocator
    // refusal a give-up names is this attempt's own.
    eng.limits().reset_meters();
    // Live vtree pointer that `batch_accumulate` and the conjunction use. Mid-compile
    // rotation search reseats this Arc; downstream ptr_eq checks require
    // every live Tdd in `state.tdds` to share the same Arc, so we propagate.
    let mut current_vtree: Arc<Vtree> = Arc::clone(vtree);
    let mut state = PassState::new(formula, vtree, vtree.num_nodes(), job);
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
                fold_step(eng, t, &ctx, &mut state, &current_vtree, None, None)
            }
        };
        if let ControlFlow::Break(out) = check_step(eng, vtree, step, &mut state, t) {
            return out;
        }
    }

    // Internal levels: conjoin child TDDs, then accumulate any clauses scoped
    // to this level. Iterating over the original outer Arc's structure is safe
    // after marginal-cluster rotations, which only swap slot content inside
    // already-consumed `subtree(t)`.
    let topo: &[VtreeIdx] = vtree.internal_bottomup_slice();
    for &t in topo {
        let step = match step_deadline(job) {
            deadline if deadline_passed(deadline) => Err(OperationError::Stopped),
            deadline => {
                let _cut = (deadline != job.deadline).then(|| deadline_scope(eng, deadline));
                run_internal_step(eng, t, &ctx, &mut state, &current_vtree)
            }
        };
        if let ControlFlow::Break(out) = check_step(eng, vtree, step, &mut state, t) {
            return out;
        }
        if let Err(e) = marginalize_and_rotate(eng, t, &ctx, &mut state, &mut current_vtree) {
            return Err(GiveUp::from_operation(eng, e));
        }
    }

    if let (Some(prune), Some(run_pass)) = (state.sat_prune.take(), job.run.sat_prune) {
        job.prune_spent_us.set(prune.compile_spent_us());
        run_pass.absorb(prune.into_stats());
    }
    let diagram = state.tdds[vtree.root().idx()].take().unwrap_or_else(|| eng.one(vtree));
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
    vtree: &Arc<Vtree>,
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
/// `Engine::rotate_marginal_cluster`). On a home this merges sibling regions
/// and leaves the home as it is.
fn marginalize_and_rotate(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    current_vtree: &mut Arc<Vtree>,
) -> Result<(), OperationError> {
    run_marginalize_at(eng, t, ctx, state, current_vtree)?;
    let PassState { tdds, cluster_tried, .. } = state;
    if ctx.job.run.projections {
        // The rotation changes the diagram's own vtree only. A Boolean
        // function has no marginal level to cluster.
        if let Some(tdd) = tdds[t.idx()].as_mut()
            && tdd.has_marginal_level()
        {
            cluster_tried.clear();
            let root = tdd.vtree().root();
            eng.rotate_marginal_cluster(tdd, root, MARGINAL_CLUSTER_BOUND_FACTOR, cluster_tried)?;
        }
        return Ok(());
    }
    if let Some(tdd) = tdds[t.idx()].as_mut() {
        eng.rotate_marginal_cluster(tdd, t, MARGINAL_CLUSTER_BOUND_FACTOR, cluster_tried)?;
    }
    propagate_vtree_arc(tdds, current_vtree, t);
    Ok(())
}

/// Reseat every TDD's `vtree` Arc to match `current_vtree` if a marginal-cluster
/// rotation just rotated the vtree held by `tdds[just_updated.idx()]`.
fn propagate_vtree_arc(
    tdds: &mut [Option<Tdd>],
    current_vtree: &mut Arc<Vtree>,
    just_updated: VtreeIdx,
) {
    let Some(tdd) = tdds[just_updated.idx()].as_ref() else { return; };
    if Arc::ptr_eq(tdd.vtree(), current_vtree) { return; }
    *current_vtree = Arc::clone(tdd.vtree());
    for (slot_idx, slot) in tdds.iter_mut().enumerate() {
        if slot_idx == just_updated.idx() { continue; }
        if let Some(td) = slot.as_mut() && !Arc::ptr_eq(td.vtree(), current_vtree) {
            // Safety: the rotation is confined to the just-completed subtree.
            // Other in-flight diagrams constrain disjoint subtrees; any retained
            // levels here are true wrappers with node-0 references. Rotation
            // preserves leaf indices, so those references remain valid.
            unsafe { td.reseat_vtree_unchecked(current_vtree); }
        }
    }
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
    current_vtree: &Arc<Vtree>,
    children: Option<Tdd>,
    lineage: Option<VtreeIdx>,
) -> Result<usize, OperationError> {
    let targets = build_marginalize_targets(t, current_vtree, ctx, &state.forgotten);
    state.minimize.fold_at_root = t == current_vtree.root();
    let throttle = lineage.map(|c| std::mem::take(&mut state.throttles[c.idx()])).unwrap_or_default();
    state.minimize.swap_throttle(throttle);
    let folded = batch_accumulate(
        eng,
        &ctx.clauses_at[t.idx()], ctx, current_vtree, children,
        state, targets.as_ref(),
    );
    state.minimize.fold_at_root = false;
    state.throttles[t.idx()] = state.minimize.swap_throttle(ContractThrottle::default());
    state.tdds[t.idx()] = folded?;
    if deadline_passed(ctx.job.deadline) { return Err(OperationError::Stopped); }
    Ok(state.tdds[t.idx()].as_ref().map_or(0, Tdd::pair_count))
}

/// Process a single internal vtree node: the cofactor step when the node
/// qualifies for it ([`cofactor_step`]), otherwise conjoin the children and
/// fold the node's clauses ([`splice_and_fold`]); then prune the node's
/// diagram when a pass fires. Returns the pair count of the node's diagram,
/// measured once by the step and again only if the pruning replaced it.
/// `Err(Stopped)` when the deadline passes during the step.
fn run_internal_step(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    current_vtree: &Arc<Vtree>,
) -> Result<usize, OperationError> {
    let mut pairs = match cofactor_step(eng, t, ctx, state, current_vtree)? {
        Some(pairs) => pairs,
        None => splice_and_fold(eng, t, ctx, state, current_vtree)?,
    };
    // Prune the node's diagram when the pass's trigger fires.
    if let Some(prune) = state.sat_prune.as_mut()
        && let Some(tdd) = state.tdds[t.idx()].as_ref()
        && prune.should_fire(trigger_size(tdd, pairs, current_vtree))
    {
        let size = trigger_size(tdd, pairs, current_vtree);
        let outcome = match ctx.job.run.work {
            Some(work) => work.budgeted(|| prune.probe_tdd(eng, tdd, size)),
            None => prune.probe_tdd(eng, tdd, size),
        };
        let pruned = match outcome {
            PruneOutcome::DeadRoot => Some(eng.zero(current_vtree)),
            PruneOutcome::Rewritten(new_tdd) => Some(*new_tdd),
            PruneOutcome::NoOp => None,
        };
        if let Some(pruned) = pruned {
            pairs = pruned.pair_count();
            state.tdds[t.idx()] = Some(pruned);
        }
    }
    Ok(pairs)
}

/// `pairs`, the pair count of `tdd`, as the SAT-prune trigger reads it: on a
/// projection of `vtree`, one pair more for each leaf of `vtree` the
/// projection lacks. On `vtree` itself the diagram holds a one-pair node on
/// each internal level the projection lacks, and there are as many of those,
/// so the trigger's thresholds, set on `vtree`, fire at the same sizes on
/// both. A level `vtree` adds between two of the projection's holds as many
/// nodes as the level under it, which this leaves out.
fn trigger_size(tdd: &Tdd, pairs: usize, vtree: &Vtree) -> usize {
    pairs + vtree.num_leaves().saturating_sub(tdd.vtree().num_leaves()) as usize
}

/// The normal internal step at `t`: conjoin the children's diagrams, by
/// [`Engine::and`] on a home and by an O(1) splice on a bare root (a weighted
/// compile), fold the node's clauses into the result ([`fold_step`]) and
/// store it in `state.tdds[t]`. Returns its pair count.
pub(super) fn splice_and_fold(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    current_vtree: &Arc<Vtree>,
) -> Result<usize, OperationError> {
    let (left, right) = current_vtree.children(t);
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
        (Some(mut l), Some(r)) => {
            if l.is_zero() || r.is_zero() {
                Some(eng.zero(current_vtree))
            } else if ctx.job.run.projections {
                // On the projections under `t`'s two children, the library
                // joins them at `t` without a sweep.
                Some(eng.and(l, r)?)
            } else {
                // The schedule marginalizes no node before its own step
                // (`marginalize_schedule`), so level `t` is still a
                // structural wrapper in both children.
                assert!(
                    !l.levels()[t.idx()].is_marginal() && !r.levels()[t.idx()].is_marginal(),
                    "level {} was marginalized before its own step",
                    t.idx(),
                );
                // The children cover disjoint variables, so their conjunction
                // creates no twins: it is the single pair at `t` naming both
                // roots, every other level moved across unchanged — an O(1)
                // splice, not an apply.
                //
                // Safety: the compiler built both operands on the current
                // shared vtree, from its disjoint left/right child scopes.
                // Their stored wrapper nodes are single pairs with a true
                // opposite side, and compilation uses one compatible weight
                // configuration. The assertion above checks the stored wrappers.
                unsafe { l.splice_subtree_unchecked(eng, r, t); }
                Some(l)
            }
        }
        (Some(t_single), None) | (None, Some(t_single)) => Some(t_single),
        (None, None) => None,
    };
    fold_step(eng, t, ctx, state, current_vtree, children_tdd, lineage)
}

#[cfg(test)]
#[path = "tests/step.rs"]
mod tests;
