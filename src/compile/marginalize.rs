//! Summing a vtree level out to its marginal count during a compile.
//!
//! `schedule.rs` decides which variables may be summed out after each step;
//! this is where that happens to the diagram. The step functions apply the
//! targets in place as they fold, and `run_marginalize_at` is the step-end
//! safety net for whatever streaming did not catch.
//!
//! A projecting compile carries its forget set in its job's output and the
//! variables forgotten so far in its pass state; both are passed in here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tididi::diagram::Tdd;
use tididi::{Engine, OperationError};
use tididi::vtree::{VarId, Vtree, VtreeIdx};

use super::schedule::StepVars;
use super::step::{PassState, StepContext};

/// What one sum-out inside a step sums: the variables of `step` that no
/// clause after position `through` of its clauses reads, `last` giving each
/// completed variable's last position; with no `last`, every variable done by
/// the step's end.
#[derive(Clone, Copy)]
pub(crate) struct Summing<'s> {
    pub(crate) step: StepVars<'s>,
    pub(crate) last: Option<&'s HashMap<VarId, u32>>,
    pub(crate) through: u32,
}

impl Summing<'_> {
    fn summable(&self, var: VarId) -> bool {
        self.step.summable(var) && self.last.and_then(|last| last.get(&var)).is_none_or(|&at| at <= self.through)
    }

    /// Sum `self` out of `tdd`.
    pub(crate) fn sum_out(self, eng: &Engine, tdd: &mut Tdd) -> Result<(), OperationError> {
        eng.marginalize(tdd, |var| self.summable(var))
    }

    /// Conjoin `a` and `b`, summing `self` out as the product is built.
    pub(crate) fn and(self, eng: &Engine, a: Tdd, b: Tdd) -> Result<Tdd, OperationError> {
        eng.and_marginalizing_where(a, b, |var| self.summable(var))
    }
}

/// What step `t` may sum out: the variables done by `t`, none while a
/// projected variable is pending, so that the ∃-forget meets no marginal
/// level on its path. Returns `None` if the pass sums nothing out or there is
/// nothing to sum out at this step.
pub(crate) fn build_marginalize_targets<'a>(
    t: VtreeIdx,
    vtree: &Vtree,
    ctx: &StepContext<'a>,
    forgotten: &HashSet<VarId>,
) -> Option<StepVars<'a>> {
    if !ctx.job.output.marginalizes() { return None; }
    let schedule = ctx.marginalize_schedule?;
    let pending = ctx.job.output.forget().is_some_and(|forget| forget.iter().any(|var| !forgotten.contains(var)));
    (schedule.frees(t) && !pending).then(|| schedule.at(vtree, t))
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
        Some(step) => Summing { step, last: None, through: 0 }.sum_out(eng, tdd),
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
    vtree: &Arc<Vtree>,
) -> Result<(), OperationError> {
    let Some(schedule) = ctx.marginalize_schedule else { return Ok(()); };
    let output = ctx.job.output;

    // Projected variables to ∃-forget at step `t`: those done at `t`, or at
    // the root step any projected variable not yet forgotten (unit-only or
    // free variables whose clause scope was a leaf). Nothing is summed out
    // while one is pending (`build_marginalize_targets`), so every returned
    // variable has a marginal-ancestor-free path.
    let mut vars_to_forget = Vec::new();
    if let Some(proj_vars) = output.forget() {
        vars_to_forget.extend(schedule.forget_at(t).iter().filter(|var| !state.forgotten.contains(var)));
        if t == vtree.root() {
            for &var in proj_vars {
                if !state.forgotten.contains(&var) && !vars_to_forget.contains(&var) {
                    vars_to_forget.push(var);
                }
            }
        }
    }
    // Nothing to marginalize and nothing to forget at this step.
    if !schedule.frees(t) && vars_to_forget.is_empty() { return Ok(()); }
    if !vars_to_forget.is_empty() && let Some(tdd_val) = state.tdds[t.idx()].take() {
        // `exists_vars` picks its own rewrite and carries the weight store to
        // its result. Every rewrite needs no marginal level on a forgotten
        // variable's path to the root, which `build_marginalize_targets` keeps.
        state.tdds[t.idx()] = Some(eng.exists_vars(tdd_val, &vars_to_forget)?);
        state.forgotten.extend(&vars_to_forget);
    }

    // The projected function sums nothing out, so a weighted fold can read
    // the whole diagram. Otherwise the forgotten variables have no leaf and
    // are free home variables, ×2 each in the count, corrected by the 2^k
    // divisor. A weighted compile marginalizes into the `WeightStore`.
    if let Some(tdd) = state.tdds[t.idx()].as_mut() {
        marginalize_scheduled_at(eng, tdd, t, vtree, ctx, &state.forgotten)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/marginalize.rs"]
mod tests;
