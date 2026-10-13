//! The cofactor step: an internal vtree node whose smaller child spans a few
//! variables folds its clauses once per assignment of those variables, into
//! the other child's diagram alone.
//!
//! At node `t` with children `small`, over the variables `S` (at most
//! [`COFACTOR_MAX_VARS`] of them), and `big`, the normal step splices the
//! children's diagrams `L` and `R` and folds the node's clauses `C_t` into the
//! result, a batch at a time ([`super::step::splice_and_fold`]). Every clause
//! of `C_t` has literals on both sides, so every merge of that fold carries the
//! `S` structure at `t` across the whole of `R`. The cofactor step fixes `S`
//! instead: for each full assignment `a` of `S` that `L` allows, it folds the
//! clauses `a` leaves unsatisfied, less their `S` literals, into a copy of `R`
//! with the ordinary fold ([`batch_accumulate`]), shortest clause first, and
//! returns the disjunction of `cube(a) ∧ R_a` over the branches that stay
//! satisfiable.
//!
//! **Soundness.** It is taken only when neither child's diagram has a marginal
//! level, so both are the exact Boolean functions of their variables (a child
//! with no diagram yet is the constant true), and `L` depends on `S` alone. A
//! full assignment `a` of `S` makes `L` a constant and leaves of `C_t` exactly
//! the unsatisfied clauses with their `S` literals removed, so the disjunction
//! of `cube(a) ∧ (L ∧ R ∧ C_t)|a` over the `a` with `L(a)` true is
//! `L ∧ R ∧ C_t` itself. Weighted compiles take the step too: a weight store
//! holds values only at marginal levels, so with none in either child the
//! branches carry no weight values, and the weights enter when the step's
//! scheduled levels are marginalized afterwards, as after the normal step.
//!
//! **Marginalization.** The branch folds marginalize nothing. The levels the
//! normal fold would sum out inside its merges ([`build_marginalize_targets`])
//! are marginalized once the branches are joined, so the step leaves the same
//! levels marginal as the normal one. The step-end pass
//! ([`super::marginalize::run_marginalize_at`]) then forgets and marginalizes
//! `t`'s schedule as after any step.
//!
//! **Cost guard.** Every branch costs at least one merge into a copy of `R` and
//! a disjunction in the join, whatever its clauses. The normal fold merges into
//! the accumulator once per batch floor's worth of clauses ([`batch_floor`] at
//! `R`), so the step is taken only when that is more merges than there are
//! branches: `ceil(|C_t| / floor) > branches`. The guard is decided from the
//! clause count, `R`'s floor and the branches `L` allows, before any fold.

use std::sync::Arc;

use tididi::diagram::Tdd;
use tididi::vtree::{VarId, Vtree, VtreeIdx, VtreeNode};
use tididi::{Engine, Literal, OperationError};

use crate::budget::deadline_passed;
use super::batch::{batch_accumulate, batch_floor};
use super::marginalize::{build_marginalize_targets, summable};
use super::step::{DiagramClauses, PassState, StepContext};

/// The most variables the smaller child of an internal vtree node may span
/// for the node to take the cofactor step, which folds the node's clauses
/// once per assignment of those variables: at most two to this power
/// branches. The assignments are bit masks in a `u8`, so it stays below
/// eight. Five rather than three lets more nodes of formulas over few
/// variables take the step, which cuts their accumulator work.
const COFACTOR_MAX_VARS: usize = 5;
const _: () = assert!(COFACTOR_MAX_VARS < 8, "the cofactor step's assignments are u8 masks");

/// The fewest clauses scoped at an internal vtree node for it to take the
/// cofactor step. A node with fewer is left to the normal fold, whose merges
/// are then few.
const COFACTOR_MIN_CLAUSES: usize = 64;

/// Take the cofactor step at internal node `t` when it qualifies, storing the
/// node's diagram in `tdds[t]` and consuming the children's slots. Returns
/// `Some` with the diagram's pair count when the step was taken, `None` (with
/// `tdds` untouched) when the node is left to the normal step.
/// `Err(Stopped)` on the deadline, checked between branches and at the end.
pub(super) fn cofactor_step(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    tdds: &mut [Option<Tdd>],
    state: &mut PassState,
    current_vtree: &Arc<Vtree>,
) -> Result<Option<usize>, OperationError> {
    let Some(plan) = plan_cofactor(eng, t, ctx, tdds, current_vtree)? else {
        return Ok(None);
    };
    run_cofactor(eng, t, ctx, tdds, state, current_vtree, plan).map(Some)
}

/// A node the cofactor step takes, worked out before any diagram is folded.
struct CofactorPlan {
    /// The child over `vars`, whose diagram (if any) only chose the branches.
    small: VtreeIdx,
    /// The other child, whose diagram every branch folds into.
    big: VtreeIdx,
    /// `S`; bit `b` of an assignment is the value of `vars[b]`.
    vars: Vec<VarId>,
    /// The assignments of `vars` the small side's diagram allows, ascending.
    branches: Vec<u8>,
    /// The node's clauses less their `S` literals, shortest first (stable).
    rest: DiagramClauses,
    /// Per clause of `rest`: the bits of its positive and of its negative `S`
    /// literals.
    masks: Vec<(u8, u8)>,
}

impl CofactorPlan {
    /// The clauses of `rest` that assignment `a` leaves unsatisfied, in
    /// `rest`'s order.
    fn open_clauses(&self, a: u8) -> Vec<usize> {
        (0..self.masks.len())
            .filter(|&i| {
                let (positive, negative) = self.masks[i];
                (positive & a) == 0 && (negative & !a) == 0
            })
            .collect()
    }
}

/// Assignment `a` of `vars` as literals.
fn assignment(vars: &[VarId], a: u8) -> impl Iterator<Item = Literal> + '_ {
    vars.iter().enumerate().map(move |(b, &var)| Literal::new(var, (a >> b) & 1 == 1))
}

/// The variables under `root`, or `None` when there are more than
/// [`COFACTOR_MAX_VARS`].
fn vars_under(vtree: &Vtree, root: VtreeIdx) -> Option<Vec<VarId>> {
    let mut vars = Vec::with_capacity(COFACTOR_MAX_VARS);
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match *vtree.node(node) {
            VtreeNode::Leaf { var, .. } => {
                if vars.len() == COFACTOR_MAX_VARS {
                    return None;
                }
                vars.push(var);
            }
            VtreeNode::Internal { left, right, .. } => {
                stack.push(left);
                stack.push(right);
            }
        }
    }
    Some(vars)
}

/// Whether node `t` takes the cofactor step, and if so its plan. `None` when
/// it has fewer than [`COFACTOR_MIN_CLAUSES`] clauses, neither child spans at
/// most [`COFACTOR_MAX_VARS`] variables, the big side has no diagram, either
/// child's diagram has a marginal level, a clause lacks literals on one side,
/// or the cost guard declines.
fn plan_cofactor(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    tdds: &[Option<Tdd>],
    vtree: &Vtree,
) -> Result<Option<CofactorPlan>, OperationError> {
    let clauses = &ctx.clauses_at[t.idx()];
    if clauses.len() < COFACTOR_MIN_CLAUSES {
        return Ok(None);
    }
    // The smaller child when both qualify, the left one on a tie.
    let (left, right) = vtree.children(t);
    let (small, big, vars) = match (vars_under(vtree, left), vars_under(vtree, right)) {
        (Some(l), Some(r)) if r.len() < l.len() => (right, left, r),
        (Some(l), _) => (left, right, l),
        (None, Some(r)) => (right, left, r),
        (None, None) => return Ok(None),
    };
    let Some(r) = tdds[big.idx()].as_ref() else { return Ok(None) };
    let l = tdds[small.idx()].as_ref();
    // The soundness condition: both children are exact Boolean functions.
    if r.has_marginal_level() || l.is_some_and(Tdd::has_marginal_level) {
        return Ok(None);
    }

    // Each clause's `S` literals as bit masks, and how many literals it has
    // on the big side. A clause scoped at `t` has both, by its scope.
    let mut masks = Vec::with_capacity(clauses.len());
    let mut rest_len = Vec::with_capacity(clauses.len());
    for &clause in clauses {
        let (mut positive, mut negative, mut rest) = (0u8, 0u8, 0usize);
        for lit in ctx.clauses.get(clause) {
            match vars.iter().position(|&var| var == lit.var) {
                Some(b) if lit.sign => positive |= 1 << b,
                Some(b) => negative |= 1 << b,
                None => rest += 1,
            }
        }
        if (positive | negative) == 0 || rest == 0 {
            return Ok(None);
        }
        masks.push((positive, negative));
        rest_len.push(rest);
    }

    let mut branches = Vec::with_capacity(1 << vars.len());
    for a in 0..1u8 << vars.len() {
        let allowed = match l {
            Some(l) => eng.is_sat(&eng.condition(l.clone(), assignment(&vars, a))?)?,
            None => true,
        };
        if allowed {
            branches.push(a);
        }
    }
    if clauses.len().div_ceil(batch_floor(Some(r)).size) <= branches.len() {
        return Ok(None);
    }

    // Shortest first, stably: on the formulas the step was developed on, the
    // branches folded in scope order cost a multiple of the work.
    let mut order: Vec<usize> = (0..clauses.len()).collect();
    order.sort_by_key(|&i| rest_len[i]);
    let mut rest = DiagramClauses::with_capacity(clauses.len(), rest_len.iter().sum());
    for &i in &order {
        rest.push(ctx.clauses.get(clauses[i]).iter().copied().filter(|lit| !vars.contains(&lit.var)));
    }
    let masks = order.iter().map(|&i| masks[i]).collect();
    Ok(Some(CofactorPlan { small, big, vars, branches, rest, masks }))
}

/// Fold `plan`'s branches into copies of the big side's diagram, join them
/// into `tdds[t]` and marginalize the levels the normal fold would have
/// summed out. Returns the node diagram's pair count.
fn run_cofactor(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    tdds: &mut [Option<Tdd>],
    state: &mut PassState,
    vtree: &Arc<Vtree>,
    plan: CofactorPlan,
) -> Result<usize, OperationError> {
    let r = tdds[plan.big.idx()].take().expect("the plan read the big side's diagram");
    // The small side's diagram has done its work: it chose the branches.
    tdds[plan.small.idx()] = None;
    // The branches fold the reduced clauses; marginalization waits for the join.
    let branch_ctx = StepContext {
        job: ctx.job,
        clauses: &plan.rest,
        clauses_at: ctx.clauses_at,
        marginalize_schedule: None,
    };
    let mut terms = Vec::with_capacity(plan.branches.len());
    for &a in &plan.branches {
        let branch = batch_accumulate(eng, &plan.open_clauses(a), &branch_ctx, vtree, Some(r.clone()), state, None)?
            .expect("a fold from a live accumulator returns one");
        // Structural, so an unsatisfiable branch is the false sentinel.
        if !branch.is_zero() {
            let cube = ctx.job.output.with_store(eng.cube(vtree, assignment(&plan.vars, a))?);
            terms.push(eng.and(cube, branch)?);
        }
        if deadline_passed(ctx.job.deadline) {
            return Err(OperationError::Stopped);
        }
    }
    drop(r);
    let mut joined = if terms.is_empty() {
        ctx.job.output.with_store(eng.zero(vtree))
    } else {
        eng.or_many(terms)?
    };
    state.minimize.adaptive_minimize(eng, &mut joined)?;
    if let Some(targets) = build_marginalize_targets(t.idx(), vtree, ctx, &state.forgotten) {
        eng.marginalize_levels(&mut joined, &summable(vtree, &targets, ctx.job.output.forget().is_some()))?;
    }
    let pairs = joined.pair_count();
    tdds[t.idx()] = Some(joined);
    if deadline_passed(ctx.job.deadline) {
        return Err(OperationError::Stopped);
    }
    Ok(pairs)
}

#[cfg(test)]
#[path = "tests/cofactor.rs"]
mod tests;
