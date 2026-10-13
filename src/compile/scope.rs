//! Clause scoping and the reordering within a node: assigns each clause to
//! its vtree LCA (`clause_scope`), groups clauses by scope node, and reorders
//! within each group (variable proximity). Produces the per-node clause
//! groups `step.rs` folds.

use tididi::vtree::{Vtree, VtreeIdx};
use vitri::cnf::{Clause, CnfFormula};

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
///
/// The tour starts at the clause with the most distinct variables, then
/// repeatedly appends the unplaced clause sharing the most variables with the
/// last placed one. Ties go to the later clause in `clauses`: the last of the
/// largest clauses starts, and the highest-indexed of the best successors
/// wins, a zero overlap included. Compilation depends on this exact order, and
/// a test pins it against a direct hash-set version.
///
/// The group's variables are renumbered densely. A group over at most 256
/// variables keeps a bitset per clause and counts an overlap as the popcount
/// of the two bitsets' intersection; a wider one keeps each clause's variable
/// list and counts it against a marker array stamped with the last clause.
/// Each step scans the unplaced clauses from the highest index down and stops
/// at the first that holds every variable of the last clause, which no lower
/// index can beat.
fn reorder_by_variable_proximity(clauses: &mut [usize], formula: &CnfFormula) {
    let sets = DenseClauseVars::new(clauses, formula);
    let order = match sets.num_vars.div_ceil(64) {
        1 => proximity_tour(BitsetRows::<1>::new(&sets), &sets.sizes),
        2 => proximity_tour(BitsetRows::<2>::new(&sets), &sets.sizes),
        3 | 4 => proximity_tour(BitsetRows::<4>::new(&sets), &sets.sizes),
        _ => proximity_tour(MarkedLists::new(&sets), &sets.sizes),
    };

    // Apply `order` to `clauses` through a reordered copy.
    let reordered: Vec<usize> = order.iter().map(|&src| clauses[src]).collect();
    clauses.copy_from_slice(&reordered);
}

/// Greedy tour over a clause group: `order[k]` is the group position that
/// should end up at position k. `sizes[i]` is clause i's distinct-variable
/// count, which bounds its overlap with any other clause.
fn proximity_tour(mut unplaced: impl Unplaced, sizes: &[u32]) -> Vec<usize> {
    let n = sizes.len();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    // `max_by_key` keeps the last maximum. Nothing is placed yet, so the
    // group position is also the position among the unplaced.
    let start = (0..n).max_by_key(|&i| sizes[i]).unwrap();
    order.push(unplaced.place(start));

    while unplaced.remaining() > 0 {
        let bound = sizes[*order.last().unwrap()];
        let top = unplaced.remaining() - 1;
        let mut best_pos = top;
        let mut best = unplaced.overlap(top);
        if best < bound {
            for pos in (0..top).rev() {
                let shared = unplaced.overlap(pos);
                if shared > best {
                    best = shared;
                    best_pos = pos;
                    if shared == bound {
                        break;
                    }
                }
            }
        }
        order.push(unplaced.place(best_pos));
    }
    order
}

/// The unplaced clauses of a proximity tour, in increasing group position,
/// and the last placed clause they are compared against.
trait Unplaced {
    fn remaining(&self) -> usize;
    /// Removes the clause at `pos` among the unplaced, keeping the rest in
    /// order, makes it the clause [`Unplaced::overlap`] compares against, and
    /// returns its group position.
    fn place(&mut self, pos: usize) -> usize;
    /// Variables the unplaced clause at `pos` shares with the last placed one.
    fn overlap(&self, pos: usize) -> u32;
}

/// A clause group's variable sets over dense variable ids `0..num_vars`.
struct DenseClauseVars {
    num_vars: usize,
    /// Clause i's distinct variables, sorted, are `vars[starts[i]..starts[i + 1]]`.
    starts: Vec<usize>,
    vars: Vec<u32>,
    /// Clause i's distinct-variable count.
    sizes: Vec<u32>,
}

impl DenseClauseVars {
    fn new(clauses: &[usize], formula: &CnfFormula) -> Self {
        let mut dense: rustc_hash::FxHashMap<u32, u32> = rustc_hash::FxHashMap::default();
        let mut starts = Vec::with_capacity(clauses.len() + 1);
        let mut vars = Vec::new();
        let mut sizes = Vec::with_capacity(clauses.len());
        let mut scratch: Vec<u32> = Vec::new();
        starts.push(0);
        for &ci in clauses {
            scratch.clear();
            for lit in &formula.clauses()[ci].literals {
                let next = dense.len() as u32;
                scratch.push(*dense.entry(lit.var.get()).or_insert(next));
            }
            scratch.sort_unstable();
            scratch.dedup();
            vars.extend_from_slice(&scratch);
            starts.push(vars.len());
            sizes.push(scratch.len() as u32);
        }
        DenseClauseVars { num_vars: dense.len(), starts, vars, sizes }
    }

    fn vars_of(&self, i: usize) -> &[u32] {
        &self.vars[self.starts[i]..self.starts[i + 1]]
    }
}

/// Unplaced clauses as `W`-word variable bitsets, kept contiguous so a scan
/// streams through them.
struct BitsetRows<const W: usize> {
    /// Group position of each unplaced clause, increasing.
    position: Vec<usize>,
    /// Variable bitset of each unplaced clause, parallel to `position`.
    rows: Vec<[u64; W]>,
    last: [u64; W],
}

impl<const W: usize> BitsetRows<W> {
    fn new(sets: &DenseClauseVars) -> Self {
        let n = sets.sizes.len();
        let rows = (0..n)
            .map(|i| {
                let mut row = [0u64; W];
                for &v in sets.vars_of(i) {
                    row[v as usize / 64] |= 1u64 << (v % 64);
                }
                row
            })
            .collect();
        BitsetRows { position: (0..n).collect(), rows, last: [0; W] }
    }
}

impl<const W: usize> Unplaced for BitsetRows<W> {
    fn remaining(&self) -> usize {
        self.position.len()
    }

    fn place(&mut self, pos: usize) -> usize {
        self.last = self.rows.remove(pos);
        self.position.remove(pos)
    }

    #[inline]
    fn overlap(&self, pos: usize) -> u32 {
        let row = &self.rows[pos];
        (0..W).map(|w| (row[w] & self.last[w]).count_ones()).sum()
    }
}

/// Unplaced clauses as variable lists, counted against a marker array that
/// holds `stamp` exactly at the last placed clause's variables.
struct MarkedLists<'a> {
    sets: &'a DenseClauseVars,
    /// Group position of each unplaced clause, increasing.
    position: Vec<usize>,
    mark: Vec<u32>,
    stamp: u32,
}

impl<'a> MarkedLists<'a> {
    fn new(sets: &'a DenseClauseVars) -> Self {
        let n = sets.sizes.len();
        MarkedLists { sets, position: (0..n).collect(), mark: vec![0; sets.num_vars], stamp: 0 }
    }
}

impl Unplaced for MarkedLists<'_> {
    fn remaining(&self) -> usize {
        self.position.len()
    }

    fn place(&mut self, pos: usize) -> usize {
        let i = self.position.remove(pos);
        self.stamp += 1;
        for &v in self.sets.vars_of(i) {
            self.mark[v as usize] = self.stamp;
        }
        i
    }

    #[inline]
    fn overlap(&self, pos: usize) -> u32 {
        let vars = self.sets.vars_of(self.position[pos]);
        vars.iter().filter(|&&v| self.mark[v as usize] == self.stamp).count() as u32
    }
}

#[cfg(test)]
#[path = "tests/scope.rs"]
mod tests;
