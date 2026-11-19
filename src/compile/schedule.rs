//! Deciding which vtree levels may be marginal after each compilation step.

use std::collections::{HashMap, HashSet};

use tididi::Literal;
use tididi::vtree::{VarId, Vtree, VtreeIdx, VtreeNode};

/// Refine one step's [`marginalize_schedule`] group down to the step's
/// clauses.
///
/// `completes_at[i]` lists the vtree nodes whose last mention among the
/// step's clauses is `clause_lits[i]`: once that clause is conjoined, no later
/// clause of the step reads those subtrees, so they can be summed out before
/// the step ends. Only the nodes the schedule frees at this step
/// (`step_targets`) are considered, the rest wait for their own step, and
/// each clause's list keeps their order.
///
/// Costs O(total clause length + number of vtree nodes) per step.
pub(crate) fn step_completions(
    clause_lits: &[&[Literal]],
    vtree: &Vtree,
    step_targets: &[VtreeIdx],
) -> Vec<Vec<VtreeIdx>> {
    let num_vars = vtree.num_vars() as usize;

    // last_clause_pos[v] = largest i where clause_lits[i] mentions v, else None.
    let mut last_clause_pos: Vec<Option<u32>> = vec![None; num_vars];
    for (i, literals) in clause_lits.iter().enumerate() {
        for lit in *literals {
            let v = lit.var.idx();
            if v < num_vars {
                last_clause_pos[v] = Some(i as u32);
            }
        }
    }

    // A subtree outside the step's node has no mention among its clauses,
    // whose variables all lie under that node, so its completion stays None.
    let completion = subtree_max(vtree, |_, var| var.and_then(|var| last_clause_pos[var.idx()]));

    let mut completes_at: Vec<Vec<VtreeIdx>> = vec![Vec::new(); clause_lits.len()];
    for &t in step_targets {
        if let Some(c) = completion[t.idx()] {
            completes_at[c as usize].push(t);
        }
    }

    completes_at
}

/// Decide which vtree levels may be marginal after each compilation step.
///
/// Returns `schedule[t]`: the vtree nodes whose levels
/// [`marginalize_levels`](tididi::Engine::marginalize_levels) may sum out once
/// step `t` completes, each group sorted bottom-up so a node's children are
/// marginal before it. A node is scheduled at the step of the highest-scoped
/// clause mentioning any variable of its subtree — after that step nothing
/// reads the subtree explicitly again — or at its own step if that is later.
/// `last_scope_pos` is [`last_scope_positions`] of the compile's clause groups.
pub(crate) fn marginalize_schedule(vtree: &Vtree, last_scope_pos: &[u32]) -> Vec<Vec<VtreeIdx>> {
    let n = vtree.num_nodes();
    let completion_pos = completion_positions(vtree, last_scope_pos);

    // Group the nodes by the step that frees them, walking the vtree
    // bottom-up so each group lists children before parents.
    let mut marginalize_at: Vec<Vec<VtreeIdx>> = vec![Vec::new(); n];
    for node in vtree.bottomup() {
        if vtree.node(node).parent().is_none() {
            continue;
        }
        let pos = completion_pos[node.idx()];
        marginalize_at[vtree.bottomup_slice()[pos as usize].idx()].push(node);
    }

    marginalize_at
}

/// Per vtree node, the topo position of the step that frees it, never before
/// the node's own step: a leaf no clause mentions completes there, and an
/// internal node whose subtree's clauses all lie below it still has its
/// children joined there.
fn completion_positions(vtree: &Vtree, last_scope_pos: &[u32]) -> Vec<u32> {
    subtree_max(vtree, |t, var| {
        let own = vtree.topo_pos(t);
        var.map_or(own, |var| last_scope_pos[var.idx()].max(own))
    })
}

/// [`marginalize_schedule`] as a predicate per variable, for a compile on a
/// home: the library sums an own node once every variable leaf under it is
/// summable, and every own node of a step's diagram lies under the step, so
/// no node is summed before its own step.
pub(crate) struct VarSchedule {
    /// Per variable (`VarId::idx`), the topo position of the step after which
    /// no clause reads it: its last scope, or its leaf's own step if later.
    done_at: Vec<u32>,
    /// Per vtree node, whether [`marginalize_schedule`] frees a node at its
    /// step; a step that frees none sums nothing.
    frees: Vec<bool>,
    /// Per vtree node, the projected variables done at its step.
    forget_at: Vec<Vec<VarId>>,
}

impl VarSchedule {
    pub(crate) fn new(vtree: &Vtree, last_scope_pos: &[u32], forget: Option<&HashSet<VarId>>) -> Self {
        let step_at = |pos: u32| vtree.bottomup_slice()[pos as usize].idx();
        let mut done_at = vec![0; vtree.num_vars() as usize];
        let mut forget_at = vec![Vec::new(); vtree.num_nodes()];
        for (t, var) in vtree.leaf_bottomup() {
            let done = last_scope_pos[var.idx()].max(vtree.topo_pos(t));
            done_at[var.idx()] = done;
            if forget.is_some_and(|forget| forget.contains(&var)) {
                forget_at[step_at(done)].push(var);
            }
        }
        let mut frees = vec![false; vtree.num_nodes()];
        for (t, &pos) in completion_positions(vtree, last_scope_pos).iter().enumerate() {
            if VtreeIdx(t as u32) != vtree.root() {
                frees[step_at(pos)] = true;
            }
        }
        Self { done_at, frees, forget_at }
    }

    /// Whether step `t` frees anything.
    pub(crate) fn frees(&self, t: VtreeIdx) -> bool {
        self.frees[t.idx()]
    }

    /// The projected variables done at step `t`.
    pub(crate) fn forget_at(&self, t: VtreeIdx) -> &[VarId] {
        &self.forget_at[t.idx()]
    }

    /// What step `t` may sum out.
    pub(crate) fn at<'a>(&'a self, vtree: &Vtree, t: VtreeIdx) -> StepVars<'a> {
        StepVars { done_at: &self.done_at, step: vtree.topo_pos(t) }
    }
}

/// The variables one step may sum out on a home: those done at or before it.
#[derive(Clone, Copy)]
pub(crate) struct StepVars<'a> {
    done_at: &'a [u32],
    step: u32,
}

impl StepVars<'_> {
    /// Whether no clause after this step reads `var`.
    pub(crate) fn summable(&self, var: VarId) -> bool {
        self.done_at[var.idx()] <= self.step
    }

    /// Whether this step holds `var`'s last clause.
    pub(crate) fn done_here(&self, var: VarId) -> bool {
        self.done_at.get(var.idx()) == Some(&self.step)
    }
}

/// [`step_completions`] for a compile on a home: the variables one step
/// completes, each with the position of its last clause among `clause_lits`.
pub(crate) fn step_last_clauses(clause_lits: &[&[Literal]], step: StepVars<'_>) -> HashMap<VarId, u32> {
    let mut last = HashMap::new();
    for (i, literals) in clause_lits.iter().enumerate() {
        for lit in *literals {
            if step.done_here(lit.var) {
                last.insert(lit.var, i as u32);
            }
        }
    }
    last
}

/// For each variable (`VarId::idx`), the bottom-up topo position of the
/// highest-scoped clause that mentions it, or 0 when no clause does: its last
/// scope. Every scope of a clause mentioning a variable is an ancestor of the
/// variable's leaf, so the highest of them is their lowest common ancestor.
/// `clauses_at[t.idx()]` lists the clauses scoped to node `t`, and
/// `vars_of(ci)` yields clause `ci`'s variables.
pub(crate) fn last_scope_positions<I: IntoIterator<Item = VarId>>(
    vtree: &Vtree,
    clauses_at: &[Vec<usize>],
    vars_of: impl Fn(usize) -> I,
) -> Vec<u32> {
    let num_vars = vtree.num_vars() as usize;
    let mut last_scope_pos: Vec<u32> = vec![0; num_vars];
    for (scope_idx, clause_indices) in clauses_at.iter().enumerate() {
        if clause_indices.is_empty() {
            continue;
        }
        let scope_pos = vtree.topo_pos(VtreeIdx(scope_idx as u32));
        for &ci in clause_indices {
            for var in vars_of(ci) {
                let v = var.idx();
                if v < num_vars {
                    last_scope_pos[v] = last_scope_pos[v].max(scope_pos);
                }
            }
        }
    }
    last_scope_pos
}

/// Bottom-up over `vtree`: each node takes the largest of `own(t, var)` —
/// `var` is the leaf's variable, `None` at an internal node — and its
/// children's values.
fn subtree_max<T: Ord + Copy + Default>(
    vtree: &Vtree,
    own: impl Fn(VtreeIdx, Option<VarId>) -> T,
) -> Vec<T> {
    let mut out = vec![T::default(); vtree.num_nodes()];
    for &t in vtree.bottomup_slice() {
        out[t.idx()] = match vtree.node(t) {
            VtreeNode::Leaf { var, .. } => own(t, Some(*var)),
            VtreeNode::Internal { left, right, .. } => own(t, None).max(out[left.idx()]).max(out[right.idx()]),
        };
    }
    out
}

#[cfg(test)]
#[path = "tests/schedule.rs"]
mod tests;
