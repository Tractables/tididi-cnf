//! Deciding which vtree levels may be marginal after each compilation step.

use tididi::Literal;
use tididi::vtree::{VarId, Vtree, VtreeIdx, VtreeNode};

/// Refine one step's [`marginalize_schedule`] group down to the individual
/// clauses of that step's batch.
///
/// `completes_at[i]` lists the vtree nodes whose last mention within the batch
/// is clause `clause_lits[i]`: once that clause is conjoined, nothing later in
/// the batch reads those subtrees, so they can be summed out mid-batch rather
/// than at the batch's end. Only nodes the cross-step schedule already freed at
/// this step (`cross_step_targets`) are considered — the rest wait for their own
/// step — and each clause's list keeps their order.
///
/// Costs O(total clause length + number of vtree nodes) per batch.
pub(crate) fn intra_batch_completions(
    clause_lits: &[&[Literal]],
    vtree: &Vtree,
    cross_step_targets: &[VtreeIdx],
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

    // Sub-vtrees outside subtree(current_node) have no in-batch mentions
    // (clauses scoped to current_node have all vars in V_{current_node}), so
    // their completion stays None.
    let completion = subtree_max(vtree, |_, var| var.and_then(|var| last_clause_pos[var.idx()]));

    let mut completes_at: Vec<Vec<VtreeIdx>> = vec![Vec::new(); clause_lits.len()];
    for &t in cross_step_targets {
        if let Some(c) = completion[t.idx()] {
            completes_at[c as usize].push(t);
        }
    }

    completes_at
}

/// Decide which vtree levels may be marginal after each compilation step.
///
/// Returns `schedule[t]`: the vtree nodes whose levels [`marginalize_levels`](tididi::Engine::marginalize_levels) may sum
/// out once step `t` completes, each group sorted bottom-up so a node's
/// children are marginal before it. A node is scheduled at the step of the
/// highest-scoped clause mentioning any variable of its subtree — after that
/// step nothing reads the subtree explicitly again — or at its own step if
/// that is later.
pub(crate) fn marginalize_schedule(
    clause_lits: &[&[Literal]],
    vtree: &Vtree,
    clauses_at: &[Vec<usize>],
) -> Vec<Vec<VtreeIdx>> {
    let n = vtree.num_nodes();

    // Step 1: for each variable, find the highest-scoped clause mentioning it.
    let last_scope_pos = last_scope_positions(clause_lits, vtree, clauses_at);

    // Step 2: compute completion_pos bottom-up, never before the node's own
    // step: a leaf no clause mentions completes there, and an internal node
    // whose subtree's clauses all lie below it still has its children joined
    // there, which needs its level structural.
    let completion_pos = subtree_max(vtree, |t, var| {
        let own = vtree.topo_pos(t);
        var.map_or(own, |var| last_scope_pos[var.idx()].max(own))
    });

    // Step 3: group the nodes by the step that frees them, walking the vtree
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

/// For each variable, the bottom-up topo position of the highest-scoped clause
/// that mentions it, or 0 when no clause does.
fn last_scope_positions(
    clause_lits: &[&[Literal]],
    vtree: &Vtree,
    clauses_at: &[Vec<usize>],
) -> Vec<u32> {
    let num_vars = vtree.num_vars() as usize;
    let mut last_scope_pos: Vec<u32> = vec![0; num_vars];
    for (scope_idx, clause_indices) in clauses_at.iter().enumerate() {
        if clause_indices.is_empty() {
            continue;
        }
        let scope_pos = vtree.topo_pos(VtreeIdx(scope_idx as u32));
        for &ci in clause_indices {
            for lit in clause_lits[ci] {
                let v = lit.var.idx();
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
