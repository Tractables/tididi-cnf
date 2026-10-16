//! Bottom-up compile of one component: [`compile_component`], the traversal
//! every compile runs, on the whole formula or once per component, and the
//! per-vtree-node leaf and internal step functions it drives. It walks the
//! vtree bottom-up, hands each scope group's clauses to [`super::batch`] to
//! fold (or, at a node whose smaller child spans a few variables, to the
//! cofactor step in [`super::cofactor`]), applies the marginalize schedule
//! through [`super::marginalize`], and stops on the deadline, or, for a
//! compile handed a [`super::StallWatch`], on a single internal step's own
//! wall ([`super::stall`]).

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::sync::Arc;

use tididi::{Engine, Literal, OperationError};
use tididi::diagram::Tdd;
use tididi::vtree::{VarId, Vtree, VtreeIdx};
use vitri::cnf::CnfFormula;

use crate::budget::deadline_passed;
use crate::convert::diagram_literal;
use super::{Compiled, GiveUp};
use super::batch::batch_accumulate;
use super::cofactor::cofactor_step;
use super::component::{CompileJob, check_vtree};
use super::marginalize::{build_marginalize_targets, run_marginalize_at};
use super::minimize::MinimizeGate;
use super::sat_prune::{PruneOutcome, SatPruneCtx};
use super::schedule::marginalize_schedule;
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
    /// Per-vtree-node lists of levels to marginalize after each step.
    /// `None` for an output that follows no schedule.
    pub(crate) marginalize_schedule: Option<&'a [Vec<VtreeIdx>]>,
}

/// What one bottom-up pass changes as it goes.
#[derive(Default)]
pub(crate) struct PassState {
    /// The adaptive-minimize gate.
    pub(crate) minimize: MinimizeGate,
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
/// clauses.
///
/// [`GiveUp::Stopped`] and [`GiveUp::Refused`] are give-ups: the pass was cut
/// short by the deadline, or an apply was refused (`OverBudget` / `OutputCap`).
/// [`GiveUp::Invalid`] is a vtree that does not place exactly the formula's
/// variables. A node proved UNSAT answers ⊥ at once, however much is left
/// uncompiled.
pub(crate) fn compile_component(
    eng: &Engine,
    formula: &CnfFormula,
    vtree: &Arc<Vtree>,
    job: CompileJob<'_>,
) -> Result<Compiled, GiveUp> {
    // Each vtree a pass runs on, whole or per component, is checked here;
    // `compile_with` checks the root for the paths that do not reach a pass.
    check_vtree(formula, vtree)?;

    // An empty clause makes the component the constant false, so it compiles
    // to ⊥ instead of reaching `clause_scope`'s assertion. Projected cutset
    // conditioning can produce one when preprocessing a conditioned UNSAT
    // branch.
    if formula.clauses().iter().any(|c| c.literals.is_empty()) {
        return Ok(Compiled { diagram: job.output.with_store(eng.zero(vtree)), forgotten: 0, peak_pairs: 0 });
    }

    // Decided before the pass starts: which clauses are conjoined at which
    // vtree node and in what order, and (for a scheduled output) when each
    // variable may be marginalized.
    let ScopeGroups { clauses_at, last_scope_pos } = group_clauses_by_scope(formula, vtree);
    let marginalize_schedule = job.output.scheduled().then(|| marginalize_schedule(vtree, &last_scope_pos));
    // Converted once for the bottom-up pass.
    let clauses = DiagramClauses::new(formula);

    // Start from state this attempt charges itself; the recovery cascade's
    // split compiles run on the same thread after it. Zero the in-flight byte
    // meter and the refused size a previous attempt left, so an allocator
    // refusal a give-up names is this attempt's own.
    eng.limits().reset_meters();
    let mut tdds: Vec<Option<Tdd>> = vec![None; vtree.num_nodes()];
    // Live vtree pointer that `batch_accumulate` and the conjunction use. Mid-compile
    // rotation search reseats this Arc; downstream ptr_eq checks require
    // every live Tdd in `tdds` to share the same Arc, so we propagate.
    let mut current_vtree: Arc<Vtree> = Arc::clone(vtree);

    // A weighted compile builds no pruning context: its marginal levels hold
    // weights, and the per-node counts a pass orders its probes by are
    // refused for them.
    let sat_prune = match job.run.sat_prune {
        Some(run_pass) if job.output.weights().is_none() => {
            run_pass.context(formula, job.run.cofactor, job.prune_spent_us.get())
        }
        _ => None,
    };
    let mut state = PassState { sat_prune, ..PassState::default() };
    state.minimize.skip_content_twins = job.run.cofactor;
    let ctx = StepContext {
        job,
        clauses: &clauses,
        clauses_at: &clauses_at,
        marginalize_schedule: marginalize_schedule.as_deref(),
    };

    // Run after each vtree node on its step's outcome and diagram: raise the
    // running peak to the pair count the step measured and continue while the
    // diagram is satisfiable. UNSAT proves the formula UNSAT and answers ⊥ at
    // once; an error aborts the pass without making a satisfiability claim.
    let mut peak_size: usize = 0;
    let mut check = |step: Result<usize, OperationError>, tdd: Option<&Tdd>| {
        let step = step.inspect(|&pairs| peak_size = peak_size.max(pairs));
        match step.and_then(|_| tdd.map_or(Ok(true), |tdd| eng.is_sat(tdd))) {
            Ok(true) => ControlFlow::Continue(()),
            Ok(false) => ControlFlow::Break(Ok(job.output.with_store(eng.zero(vtree)))),
            Err(error) => ControlFlow::Break(Err(GiveUp::from_operation(eng, error))),
        }
    };

    // The internal schedule, read once: the internal loop below walks it.
    let topo: &[VtreeIdx] = vtree.internal_bottomup_slice();

    // Leaf levels: accumulate clauses local to this leaf (never has children).
    // Each step, leaf or internal, first tests the deadline; the deadline or
    // an apply error aborts the pass, and the caller decides what runs next.
    for (t, _var) in vtree.leaf_bottomup() {
        let step = if deadline_passed(ctx.job.deadline) {
            Err(OperationError::Stopped)
        } else {
            fold_step(eng, t, &ctx, &mut tdds, &mut state, &current_vtree, None)
        };
        if let ControlFlow::Break(out) = check(step, tdds[t.idx()].as_ref()) {
            return out.map(|diagram| Compiled { diagram, forgotten: state.forgotten.len(), peak_pairs: peak_size });
        }
    }

    // The cluster rotations tried so far, kept across steps so that each is
    // tried once in the compile (see `Engine::rotate_marginal_cluster`).
    let mut cluster_tried: Vec<u8> = Vec::new();
    // Internal levels: conjoin child TDDs, then accumulate any clauses scoped
    // to this level.
    //
    // Iterating over the *original* outer Arc's structure is safe even after
    // marginal-cluster rotations: rotations only swap slot content at indices inside
    // subtree(t) for the just-processed t, and t plus all its descendants are
    // already consumed. Future iteration items are strict ancestors of t,
    // whose (left, right) entries are unchanged.
    for (k, &t) in topo.iter().enumerate() {
        // The step's own wall, when the run's stall watch bounds it: this
        // step, its satisfiability check and the marginalization after it run
        // under a deadline cut to the step's start plus the wall, the job's
        // included, which the step's own checks read. A stop while it is up
        // is the watch's to record ([`WalledStep::note_stop`]).
        let walled =
            ctx.job.run.stall_watch.and_then(|watch| watch.start_step(eng, t, k, topo.len(), ctx.job.deadline));
        let stopped = |give_up: GiveUp| {
            if let (GiveUp::Stopped, Some(step)) = (&give_up, &walled) {
                step.note_stop();
            }
            give_up
        };
        let walled_ctx = walled.as_ref().map(|step| StepContext {
            job: CompileJob { deadline: Some(step.deadline), ..ctx.job },
            ..ctx
        });
        let ctx = walled_ctx.as_ref().unwrap_or(&ctx);
        let step = if deadline_passed(ctx.job.deadline) {
            Err(OperationError::Stopped)
        } else {
            run_internal_step(eng, t, ctx, &mut tdds, &mut state, &current_vtree)
        };
        if let ControlFlow::Break(out) = check(step, tdds[t.idx()].as_ref()) {
            return out
                .map(|diagram| Compiled { diagram, forgotten: state.forgotten.len(), peak_pairs: peak_size })
                .map_err(stopped);
        }
        // Then the scheduled marginalization, and the targeted
        // marginal-clustering rotation pass: after step `t` marginalized levels
        // in subtree(t), try rotations that bring two marginal levels under one
        // parent so it too collapses to counts, shrinking the in-flight diagram
        // (peak), not just the final size. Confined to subtree(t);
        // count-preserving. See `Engine::rotate_marginal_cluster`. Either can
        // fail (the marginalization can be refused memory), and a
        // failure aborts the pass like one from the step above: a cut leaves
        // the accumulator count-exact, but there is no wall left to finish
        // the step.
        let marginalized = run_marginalize_at(eng, t, ctx, &mut state.forgotten, &mut tdds, &current_vtree)
        .and_then(|()| match tdds[t.idx()].as_mut() {
            Some(tdd) => eng
                .rotate_marginal_cluster(tdd, t, MARGINAL_CLUSTER_BOUND_FACTOR, &mut cluster_tried)
                .map(drop),
            None => Ok(()),
        });
        if let Err(e) = marginalized {
            return Err(stopped(GiveUp::from_operation(eng, e)));
        }
        propagate_vtree_arc(&mut tdds, &mut current_vtree, t);
    }

    // The context's unproductive spend joins the compile's, which the next
    // component's context starts from. Every other exit of the pass ends the
    // compile, so no later context could read it.
    if let (Some(prune), Some(run_pass)) = (state.sat_prune.take(), job.run.sat_prune) {
        job.prune_spent_us.set(prune.compile_spent_us());
        run_pass.absorb(prune.into_stats());
    }

    let diagram = tdds[vtree.root().idx()].take().unwrap_or_else(|| job.output.with_store(eng.one(vtree)));
    Ok(Compiled { diagram, forgotten: state.forgotten.len(), peak_pairs: peak_size })
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
/// `None`), marginalizing in the fold, and store the result in `tdds[t]`.
/// Returns its pair count, 0 when the node has no diagram. `Err(Stopped)`
/// when the deadline passes during the fold.
fn fold_step(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    tdds: &mut [Option<Tdd>],
    state: &mut PassState,
    current_vtree: &Arc<Vtree>,
    children: Option<Tdd>,
) -> Result<usize, OperationError> {
    let targets = build_marginalize_targets(t.idx(), current_vtree, ctx, &state.forgotten);
    tdds[t.idx()] = batch_accumulate(
        eng,
        &ctx.clauses_at[t.idx()], ctx, current_vtree, children,
        state, targets.as_deref(),
    )?;
    if deadline_passed(ctx.job.deadline) { return Err(OperationError::Stopped); }
    Ok(tdds[t.idx()].as_ref().map_or(0, Tdd::pair_count))
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
    tdds: &mut [Option<Tdd>],
    state: &mut PassState,
    current_vtree: &Arc<Vtree>,
) -> Result<usize, OperationError> {
    let mut pairs = match cofactor_step(eng, t, ctx, tdds, state, current_vtree)? {
        Some(pairs) => pairs,
        None => splice_and_fold(eng, t, ctx, tdds, state, current_vtree)?,
    };
    // Prune the node's diagram when the pass's trigger fires.
    if let Some(prune) = state.sat_prune.as_mut()
        && let Some(tdd) = tdds[t.idx()].as_ref()
        && prune.should_fire(pairs)
    {
        let pruned = match prune.probe_tdd(eng, tdd, pairs) {
            PruneOutcome::DeadRoot => Some(eng.zero(current_vtree)),
            PruneOutcome::Rewritten(new_tdd) => Some(*new_tdd),
            PruneOutcome::NoOp => None,
        };
        if let Some(pruned) = pruned {
            pairs = pruned.pair_count();
            tdds[t.idx()] = Some(pruned);
        }
    }
    Ok(pairs)
}

/// The normal internal step at `t`: conjoin the children's diagrams by an
/// O(1) splice, fold the node's clauses into the result ([`fold_step`]) and
/// store it in `tdds[t]`. Returns its pair count.
pub(super) fn splice_and_fold(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    tdds: &mut [Option<Tdd>],
    state: &mut PassState,
    current_vtree: &Arc<Vtree>,
) -> Result<usize, OperationError> {
    let (left, right) = current_vtree.children(t);
    let children_tdd = match (tdds[left.idx()].take(), tdds[right.idx()].take()) {
        (Some(mut l), Some(r)) => {
            if l.is_zero() || r.is_zero() {
                Some(ctx.job.output.with_store(eng.zero(current_vtree)))
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
    fold_step(eng, t, ctx, tdds, state, current_vtree, children_tdd)
}

#[cfg(test)]
#[path = "tests/step.rs"]
mod tests;
