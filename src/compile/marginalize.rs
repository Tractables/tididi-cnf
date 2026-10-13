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
use std::collections::HashSet;
use std::sync::Arc;

use tididi::Engine;
use tididi::OperationError;
use tididi::diagram::Tdd;
use tididi::vtree::{VarId, Vtree, VtreeIdx, VtreeNode};

use super::step::StepContext;

// ── `forget_shield` ───────────────────────────────────────

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
        if v.idx() >= vtree.num_vars() as usize { continue; }
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

// ── `build_marginalize_targets` ───────────────────────────────────────────────

/// The vtree nodes newly-saveable at step `t` — `marginalize_schedule[t.idx()]`
/// minus the projection shield — in the schedule's order, children before
/// parents. Returns `None` if the pass sums nothing out or nothing is saveable
/// at this step.
///
/// The projection shield ([`forget_shield`]) is left out,
/// so the levels a pending ∃-forget needs stay explicit; they remain in the
/// full schedule, and `run_marginalize_at` marginalizes them once their
/// variables are forgotten.
pub(crate) fn build_marginalize_targets<'a>(
    t_idx: usize,
    vtree: &Vtree,
    ctx: &StepContext<'a>,
    forgotten: &HashSet<VarId>,
) -> Option<Cow<'a, [VtreeIdx]>> {
    if !ctx.job.output.marginalizes() { return None; }
    let schedule = ctx.marginalize_schedule?;
    let kept = unshielded(&schedule[t_idx], vtree, ctx.job.output.forget(), forgotten);
    if kept.is_empty() { None } else { Some(kept) }
}

// ── `run_marginalize_at` ──────────────────────────────────────────────────────

/// What step `t` leaves for after its fold:
/// - ∃-forget each projected variable whose leaf is scheduled at `t`.
/// - Marginalize every scheduled level the fold did not sum out. A level that
///   is already marginal, empty or a leaf is left as it is, so where the fold
///   summed everything out this does nothing; it is not an assertion because a
///   SAT pruning pass can leave a scheduled level explicit. The marginalization
///   runs its own fusion sweep at the parents of the levels it collapses.
pub(crate) fn run_marginalize_at(
    eng: &Engine,
    t: VtreeIdx,
    ctx: &StepContext<'_>,
    forgotten: &mut HashSet<VarId>,
    tdds: &mut [Option<Tdd>],
    current_vtree: &Arc<Vtree>,
) -> Result<(), OperationError> {
    let Some(schedule) = ctx.marginalize_schedule else { return Ok(()); };
    let output = ctx.job.output;

    // Forget each projected var at the step where its own leaf is scheduled to
    // marginalize (completion_pos[leaf(x)]) — the earliest sound point. This
    // forget runs before the marginalize below, and completion_pos is monotone
    // up the vtree, so every ancestor of x has completion_pos >= t and is still
    // explicit (un-marginalized) at this instant — the structural ∃-forget's
    // no-marginal-ancestor precondition holds by construction. Triggering on the
    // leaf (not the leaf-parent) also covers projected vars whose leaf-parent is
    // the vtree root, which is never scheduled. The projected level keeps a
    // One on the leaf side, so the marginalize counts ×2 per projected var —
    // corrected by the caller via 2^k.
    let mut vars_to_forget: Vec<VarId> = Vec::new();
    if let Some(proj_vars) = output.forget() {
        // (1) Primary trigger: a projected var whose own leaf is scheduled to
        // marginalize at step t (= clause-scope is the internal node t).
        for &d in &schedule[t.idx()] {
            if let VtreeNode::Leaf { var, .. } = *current_vtree.node(d)
                && proj_vars.contains(&var) && !forgotten.contains(&var)
            {
                vars_to_forget.push(var);
            }
        }
        // (2) Root backstop: any projected var not yet forgotten by (1) — its
        // clause-scope was a leaf-step the main loop never visits (unit-only
        // or free vars, incl. root-children) — is forgotten when the root
        // step is processed. The projection shield (`forget_shield`) keeps
        // every not-yet-forgotten projected var's ancestors explicit, so at
        // the root every such var still has a marginal-ancestor-free path —
        // the structural ∃-forget's precondition holds. (Free vars are ×2-correct without a forget, but
        // ∃ on an unconstrained leaf is a harmless no-op and keeps the
        // uniform 2^k divisor valid.)
        if t == current_vtree.root() {
            for &var in proj_vars {
                if var.idx() < current_vtree.num_vars() as usize
                    && !forgotten.contains(&var)
                    && !vars_to_forget.contains(&var)
                {
                    vars_to_forget.push(var);
                }
            }
        }
    }
    // Nothing to marginalize and nothing to forget at this step.
    if schedule[t.idx()].is_empty() && vars_to_forget.is_empty() { return Ok(()); }
    if !vars_to_forget.is_empty() && let Some(tdd_val) = tdds[t.idx()].take() {
        // `exists_vars` picks its own rewrite and carries the weight store to
        // its result. Every rewrite needs no marginal level on a forgotten
        // variable's path to the root, which the projection shield keeps.
        tdds[t.idx()] = Some(eng.exists_vars(tdd_val, &vars_to_forget)?);
        forgotten.extend(&vars_to_forget);
    }

    // The projected function leaves its forgotten leaves as structural One
    // and sums nothing out, so a weighted fold can read the whole diagram.
    if !output.marginalizes() {
        return Ok(());
    }

    if let Some(tdd) = tdds[t.idx()].as_mut() {
        // Projection: keep the shield of still-unforgotten projected vars out
        // of this marginalization, as `build_marginalize_targets` does for the
        // fold. Otherwise an ancestor goes marginal before its projected
        // descendant is ∃-forgotten and the structural ∃-forget asserts. The
        // forgotten vars themselves are marginalized here (their leaf level is
        // now a One — ×2, corrected by the 2^k divisor). Without projection
        // the schedule is borrowed as it is.
        let batch = unshielded(&schedule[t.idx()], current_vtree, output.forget(), forgotten);
        // A weighted compile marginalizes into the WeightStore.
        //
        // `marginalize` restores its own invariants: the fusion sweep at the
        // parents of the batch and the slot prune are its epilogue, including
        // the log-domain rule that skips the sweep. It passes through a
        // `Stopped` cut inside the batch, which the step aborts on like a cut
        // inside the apply before it.
        eng.marginalize_levels(tdd, &summable(current_vtree, &batch, output.forget().is_some()))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/marginalize.rs"]
mod tests;
