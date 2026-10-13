//! Clause scoping and intra-batch reordering: assigns each clause to its vtree
//! LCA (`clause_scope`), groups clauses by scope node, and reorders within each
//! group (variable-proximity). Produces the per-node clause batches consumed
//! by `step.rs`.
use vitri::cnf::{Clause, CnfFormula};
use tididi::vtree::{Vtree, VtreeIdx};

use crate::convert::diagram_var;

/// Lowest vtree node whose subtree contains all variables in a clause (LCA).
fn clause_scope(vtree: &Vtree, clause: &Clause) -> VtreeIdx {
    assert!(!clause.literals.is_empty(), "clause_scope: empty clause = ⊥");
    clause.literals.iter()
        .map(|lit| {
            vtree
                .leaf_of(diagram_var(lit.var))
                .expect("the vtree carries this variable")
        })
        .reduce(|a, b| vtree.lca(a, b))
        .unwrap()
}

/// Group the formula's clause indices by their vtree scope (the LCA of their
/// variables).
///
/// Returns a Vec indexed by VtreeIdx, where each entry is a list of clause
/// indices whose scope is that vtree node.
///
/// The proximity reorder is a permutation of each scope group, so it cannot
/// change the conjunction the group contributes — only the intermediate
/// diagrams along the way.
pub(crate) fn group_clauses_by_scope(
    formula: &CnfFormula,
    vtree: &Vtree,
) -> Vec<Vec<usize>> {
    let mut clauses_at: Vec<Vec<usize>> = vec![Vec::new(); vtree.num_nodes()];
    for (i, clause) in formula.clauses().iter().enumerate() {
        clauses_at[clause_scope(vtree, clause).idx()].push(i);
    }

    for group in clauses_at.iter_mut().filter(|group| group.len() > 2) {
        reorder_by_variable_proximity(group, formula);
    }

    clauses_at
}

/// Reorder clause indices by variable-overlap proximity (greedy nearest-neighbor).
/// Clauses that share variables are placed adjacent to improve compilation locality.
fn reorder_by_variable_proximity(clauses: &mut [usize], formula: &CnfFormula) {
    use rustc_hash::FxHashSet;
    let n = clauses.len();
    // Variable set for each clause, indexed by position in `clauses`.
    let clause_vars: Vec<FxHashSet<u32>> = clauses.iter()
        .map(|&ci| formula.clauses()[ci].literals.iter().map(|l| l.var.get()).collect())
        .collect();

    // Greedy tour: `new_order[k]` is the source index (into `clauses`) that
    // should end up at position k. Start with the clause that has the most
    // variables — it's likely to have the most overlap with other clauses.
    let mut placed = vec![false; n];
    let mut new_order: Vec<usize> = Vec::with_capacity(n);

    let start = (0..n).max_by_key(|&i| clause_vars[i].len()).unwrap();
    placed[start] = true;
    new_order.push(start);

    while new_order.len() < n {
        let last = *new_order.last().unwrap();
        let last_vs = &clause_vars[last];
        let best = (0..n)
            .filter(|&i| !placed[i])
            .max_by_key(|&i| clause_vars[i].intersection(last_vs).count())
            .unwrap();
        placed[best] = true;
        new_order.push(best);
    }

    // Apply `new_order` to `clauses` via a reordered copy. A straightforward
    // copy here is simpler than cycle-following in-place permutation and
    // uses only one extra Vec of `usize`.
    let reordered: Vec<usize> = new_order.iter().map(|&src| clauses[src]).collect();
    clauses.copy_from_slice(&reordered);
}
