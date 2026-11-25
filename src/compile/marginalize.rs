//! Summing a vtree level out to its marginal count during a compile.
//!
//! `schedule.rs` decides which levels may be summed out after each step; this
//! is where that happens to the diagram. The step functions apply the targets
//! in place as they fold, `run_marginalize_at` is the step-end safety net for
//! whatever streaming did not catch, and the projection shield is the one
//! statement of which subtrees must not be summed out yet because an
//! un-forgotten projected variable still reads them.
//!
//! A projecting compile carries its forget set in its job's output and the
//! variables forgotten so far in its pass state; both are passed in here.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tididi::diagram::Tdd;
use tididi::{Engine, OperationError};
use tididi::vtree::{VarId, Vtree, VtreeIdx, VtreeNode};

use super::component::PassOutput;
use super::schedule::{StepVars, VarSchedule, marginalize_schedule};
use super::step::{PassState, StepContext};

/// The projection shield: the vtree nodes that must stay explicit while a
/// projected variable is not yet ∃-forgotten, namely its ancestors and the
/// whole path-sibling subtree at each ancestor. The structural ∃-forget copies
/// sibling refs verbatim and its owner-class regroup assumes distinct sibling
/// refs are mutually exclusive; marginalizing a sibling subtree breaks that
/// (distinct nodes then carry overlapping marginal count-aggregates) and the
/// regroup miscounts. The ancestors must stay explicit or the ∃-forget meets a
/// marginal ancestor. Variables in `forgotten` are left out, so the shield
/// releases as the compile forgets them; without that a mostly projected
/// formula would shield the whole tree and never marginalize.
fn forget_shield(
    vtree: &Vtree,
    forget: &HashSet<VarId>,
    forgotten: &HashSet<VarId>,
) -> HashSet<VtreeIdx> {
    let mut skip = HashSet::new();
    for v in forget {
        if forgotten.contains(v) { continue; }
        let leaf_idx = vtree.leaf_of(*v).expect("the vtree carries this variable");
        // Walk leaf→root: shield each ancestor, and the whole subtree of
        // each lower path node's sibling (the pair's sibling-side child).
        let mut cur = leaf_idx;
        while let Some(parent) = vtree.node(cur).parent() {
            skip.insert(parent);
            let sib = vtree.sibling(cur);
            // Collect the entire sibling subtree.
            let mut stack = vec![sib];
            while let Some(n) = stack.pop() {
                if !skip.insert(n) { continue; }
                if !vtree.node(n).is_leaf() {
                    let (l, r) = vtree.children(n);
                    stack.push(l);
                    stack.push(r);
                }
            }
            cur = parent;
        }
    }
    skip
}

/// `targets` less the projection shield of `forget`, in their order.
fn unshielded<'a>(
    targets: &'a [VtreeIdx],
    vtree: &Vtree,
    forget: Option<&HashSet<VarId>>,
    forgotten: &HashSet<VarId>,
) -> Cow<'a, [VtreeIdx]> {
    let Some(forget) = forget else { return targets.into(); };
    let skip = forget_shield(vtree, forget, forgotten);
    if skip.is_empty() { return targets.into(); }
    targets.iter().copied().filter(|d| !skip.contains(d)).collect()
}

/// The `targets` a marginalization may sum out now. A projecting compile
/// keeps every leaf level Boolean, so a later ∃-forget can cofactor its label;
/// internal targets stay eligible, since their folds read the leaf labels
/// directly.
pub(crate) fn summable<'a>(
    vtree: &Vtree,
    targets: &'a [VtreeIdx],
    projecting: bool,
) -> Cow<'a, [VtreeIdx]> {
    if projecting {
        Cow::Owned(targets.iter().copied().filter(|&t| !vtree.node(t).is_leaf()).collect())
    } else {
        Cow::Borrowed(targets)
    }
}

/// The schedule a pass follows ([`StepContext::marginalize_schedule`]).
pub(crate) enum Schedule {
    /// On a bare root: per step, the nodes it frees ([`marginalize_schedule`]).
    Nodes(Vec<Vec<VtreeIdx>>),
    /// On a home: per variable, the step after which nothing reads it.
    Vars(VarSchedule),
}

impl Schedule {
    /// The schedule of a pass on `vtree` whose output is `output`.
    pub(crate) fn new(vtree: &Vtree, last_scope_pos: &[u32], output: PassOutput<'_>, home: bool) -> Self {
        match home {
            true => Schedule::Vars(VarSchedule::new(vtree, last_scope_pos, output.forget())),
            false => Schedule::Nodes(marginalize_schedule(vtree, last_scope_pos)),
        }
    }
}

/// What one step may sum out.
#[derive(Clone)]
pub(crate) enum StepTargets<'a> {
    /// Nodes of a bare root, children before parents.
    Nodes(Cow<'a, [VtreeIdx]>),
    /// Variables of a home.
    Vars(StepVars<'a>),
}

/// What one sum-out inside a step sums.
#[derive(Clone, Copy)]
pub(crate) enum Summing<'s> {
    /// Nodes of a bare root, children before parents.
    Nodes(&'s [VtreeIdx]),
    /// The variables of `step` that no clause after position `through` of
    /// its clauses reads, `last` giving each completed variable's last
    /// position; with no `last`, every variable done by the step's end.
    Vars { step: StepVars<'s>, last: Option<&'s HashMap<VarId, u32>>, through: u32 },
}

impl Summing<'_> {
    fn summable(&self, var: VarId) -> bool {
        let Summing::Vars { step, last, through } = self else { unreachable!("a bare root names nodes") };
        step.summable(var) && last.and_then(|last| last.get(&var)).is_none_or(|&at| at <= *through)
    }

    /// Whether `self` names nothing to sum out. A `Vars` sum is built only
    /// once a clause completes a variable, so it is never empty, though a
    /// projecting compile may still sum nothing at it.
    pub(crate) fn is_empty(&self) -> bool {
        matches!(self, Summing::Nodes(targets) if targets.is_empty())
    }

    /// Sum `self` out of `tdd`.
    pub(crate) fn sum_out(self, eng: &Engine, tdd: &mut Tdd) -> Result<(), OperationError> {
        match self {
            Summing::Nodes(targets) => eng.marginalize_levels(tdd, targets),
            Summing::Vars { .. } => eng.marginalize(tdd, |var| self.summable(var)),
        }
    }

    /// Conjoin `a` and `b`, summing `self` out as the product is built.
    pub(crate) fn and(self, eng: &Engine, a: Tdd, b: Tdd) -> Result<Tdd, OperationError> {
        match self {
            Summing::Nodes(targets) => eng.and_marginalizing(a, b, targets),
            Summing::Vars { .. } => eng.and_marginalizing_where(a, b, |var| self.summable(var)),
        }
    }
}

/// What step `t` may sum out: on a bare root `marginalize_schedule[t]` minus
/// the projection shield, in the schedule's order, children before parents;
/// on a home the variables done by `t`, none while a projected variable is
/// pending, since its shield is every node but its own leaf. Returns `None`
/// if the pass sums nothing out or there is nothing to sum out at this step.
///
/// The projection shield ([`forget_shield`]) is left out,
/// so the levels a pending ∃-forget needs stay explicit; they remain in the
/// full schedule, and `run_marginalize_at` marginalizes them once their
/// variables are forgotten.
pub(crate) fn build_marginalize_targets<'a>(
    t: VtreeIdx,
    vtree: &Vtree,
    ctx: &StepContext<'a>,
    forgotten: &HashSet<VarId>,
) -> Option<StepTargets<'a>> {
    if !ctx.job.output.marginalizes() { return None; }
    match ctx.marginalize_schedule? {
        Schedule::Nodes(schedule) => {
            let kept = unshielded(&schedule[t.idx()], vtree, ctx.job.output.forget(), forgotten);
            if kept.is_empty() { None } else { Some(StepTargets::Nodes(kept)) }
        }
        Schedule::Vars(schedule) => {
            let pending = ctx.job.output.forget().is_some_and(|forget| forget.iter().any(|var| !forgotten.contains(var)));
            (schedule.frees(t) && !pending).then(|| StepTargets::Vars(schedule.at(vtree, t)))
        }
    }
}

/// Sum out what step `t` may sum out ([`build_marginalize_targets`]) that is
/// still explicit.
pub(crate) fn marginalize_scheduled_at(
    eng: &Engine,
    tdd: &mut Tdd,
    t: VtreeIdx,
    vtree: &Vtree,
    ctx: &StepContext<'_>,
    forgotten: &HashSet<VarId>,
) -> Result<(), OperationError> {
    match build_marginalize_targets(t, vtree, ctx, forgotten) {
        Some(StepTargets::Nodes(targets)) => {
            Summing::Nodes(&summable(vtree, &targets, ctx.job.output.forget().is_some())).sum_out(eng, tdd)
        }
        Some(StepTargets::Vars(step)) => Summing::Vars { step, last: None, through: 0 }.sum_out(eng, tdd),
        None => Ok(()),
    }
}

/// What step `t` does after its fold:
/// - ∃-forget each projected variable whose leaf is scheduled at `t`.
/// - Marginalize every scheduled level the fold did not sum out. A level that
///   is already marginal, empty or a leaf is left as it is, so where the fold
///   summed everything out this does nothing.
pub(crate) fn run_marginalize_at(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    state: &mut PassState,
    current_vtree: &Arc<Vtree>,
) -> Result<(), OperationError> {
    let Some(schedule) = ctx.marginalize_schedule else { return Ok(()); };
    let output = ctx.job.output;

    // Projected variables to ∃-forget at step `t`: those whose own leaf is
    // scheduled to marginalize at `t` (`completion_pos[leaf(x)]`), or at the
    // root step any projected variable not yet forgotten (unit-only or free
    // variables whose clause scope was a leaf). Because `completion_pos` is
    // monotone up the vtree and the projection shield keeps unforgotten
    // projected variables' ancestors explicit, every returned variable has a
    // marginal-ancestor-free path.
    let mut vars_to_forget = Vec::new();
    if let Some(proj_vars) = output.forget() {
        match schedule {
            Schedule::Nodes(schedule) => {
                for &d in &schedule[t.idx()] {
                    if let VtreeNode::Leaf { var, .. } = *current_vtree.node(d)
                        && proj_vars.contains(&var) && !state.forgotten.contains(&var)
                    {
                        vars_to_forget.push(var);
                    }
                }
            }
            Schedule::Vars(schedule) => {
                vars_to_forget.extend(schedule.forget_at(t).iter().filter(|var| !state.forgotten.contains(var)));
            }
        }
        if t == current_vtree.root() {
            for &var in proj_vars {
                if !state.forgotten.contains(&var) && !vars_to_forget.contains(&var) {
                    vars_to_forget.push(var);
                }
            }
        }
    }
    let frees = match schedule {
        Schedule::Nodes(schedule) => !schedule[t.idx()].is_empty(),
        Schedule::Vars(schedule) => schedule.frees(t),
    };
    // Nothing to marginalize and nothing to forget at this step.
    if !frees && vars_to_forget.is_empty() { return Ok(()); }
    if !vars_to_forget.is_empty() && let Some(tdd_val) = state.tdds[t.idx()].take() {
        // `exists_vars` picks its own rewrite and carries the weight store to
        // its result. Every rewrite needs no marginal level on a forgotten
        // variable's path to the root, which the projection shield keeps.
        state.tdds[t.idx()] = Some(eng.exists_vars(tdd_val, &vars_to_forget)?);
        state.forgotten.extend(&vars_to_forget);
    }

    // The projected function leaves its forgotten leaves as structural One
    // and sums nothing out, so a weighted fold can read the whole diagram.
    // Otherwise, keep the shield of still-unforgotten projected vars out of
    // this marginalization (`build_marginalize_targets`), while marginalizing
    // the forgotten vars themselves (on a bare root their leaf level is now a
    // One — ×2, corrected by the 2^k divisor; on a home they have no leaf and
    // are free home variables, ×2 likewise). A weighted compile marginalizes
    // into the `WeightStore`, and `marginalize_levels` passes a `Stopped` cut
    // through.
    if let Some(tdd) = state.tdds[t.idx()].as_mut() {
        marginalize_scheduled_at(eng, tdd, t, current_vtree, ctx, &state.forgotten)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/marginalize.rs"]
mod tests;
