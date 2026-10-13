//! Clause scoping and the clause order within a node: assigns each clause to
//! its vtree LCA (`clause_scope`), groups clauses by scope node, and orders
//! each group (`order_group`: the variable-proximity tour, or the completion
//! order where that is clearly smaller). Produces the per-node clause groups
//! `step.rs` folds.

use tididi::vtree::{Vtree, VtreeIdx};
use vitri::cnf::{Clause, CnfFormula, VarId};

use crate::convert::diagram_var;
use super::schedule::last_scope_positions;

/// Clause groups larger than this keep the proximity tour without computing
/// the completion order ([`order_group`]): the completion greedy scans every
/// unplaced clause at each placement, quadratic in the group.
const COMPLETION_ORDER_MAX_CLAUSES: usize = 4096;

/// At a node where some variable finishes, the completion order replaces the
/// proximity tour only when it is clearly smaller ([`prefer_completion`]):
/// its largest open count at most this fraction of the tour's, and its summed
/// open count at most [`COMPLETION_ORDER_SUM_RATIO`] of the tour's. The
/// margin keeps the tour on near-ties, where the completion order can score a
/// little smaller and still build far larger diagrams: bipartite stars of
/// two-literal clauses, where it opens one side's hubs before it finishes
/// any. The profile is only a proxy for the diagrams' size, so the order is
/// changed only where the proxy says so clearly.
const COMPLETION_ORDER_MAX_RATIO: f64 = 0.9;

/// The completion order's summed open count, as a fraction of the tour's,
/// above which a node where some variable finishes keeps the tour; see
/// [`COMPLETION_ORDER_MAX_RATIO`]. A node where no variable finishes needs no
/// margin: both orders end with every variable open, so their maxima tie and
/// only the order of touching differs, and a smaller sum is enough.
const COMPLETION_ORDER_SUM_RATIO: f64 = 0.8;

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

/// The compile's clause groups: which clauses fold at which vtree node, in
/// what order, and the node each variable is last mentioned at.
pub(crate) struct ScopeGroups {
    /// `clauses_at[t.idx()]`: the indices of the clauses scoped to node t, in
    /// the order the node folds them.
    pub(crate) clauses_at: Vec<Vec<usize>>,
    /// Per variable (`VarId::idx` of the diagram's variable), the bottom-up
    /// topo position of its last scope ([`last_scope_positions`]): the highest
    /// scope of a clause mentioning it, after whose step no clause reads it.
    pub(crate) last_scope_pos: Vec<u32>,
}

/// Group the formula's clause indices by their vtree scope (the LCA of their
/// variables), and order each group of more than two clauses
/// ([`order_group`]).
///
/// The order is a permutation of each scope group, so it cannot change the
/// conjunction the group contributes, only the intermediate diagrams along
/// the way.
pub(crate) fn group_clauses_by_scope(formula: &CnfFormula, vtree: &Vtree) -> ScopeGroups {
    let mut clauses_at: Vec<Vec<usize>> = vec![Vec::new(); vtree.num_nodes()];
    for (i, clause) in formula.clauses().iter().enumerate() {
        clauses_at[clause_scope(vtree, clause).idx()].push(i);
    }

    let last_scope_pos = last_scope_positions(vtree, &clauses_at, |ci| {
        formula.clauses()[ci].literals.iter().map(|lit| diagram_var(lit.var))
    });
    let mut scratch = OrderScratch::default();
    for (t, group) in clauses_at.iter_mut().enumerate().filter(|(_, group)| group.len() > 2) {
        // A variable finishes here when this node is its last scope.
        let here = vtree.topo_pos(VtreeIdx(t as u32));
        let finishes_here = |var: VarId| last_scope_pos[diagram_var(var).idx()] == here;
        order_group(group, formula, finishes_here, &mut scratch);
    }

    ScopeGroups { clauses_at, last_scope_pos }
}

/// Order one scope group in place for its node's fold.
///
/// Two orders compete: the proximity tour ([`proximity_order`]) and the
/// completion order ([`OrderScratch::completion_order`]). Each is scored by
/// its [`OpenProfile`], and the completion order replaces the tour only where
/// [`prefer_completion`] finds it clearly smaller. `finishes_here(v)` says
/// whether this node is v's last scope. A group over
/// [`COMPLETION_ORDER_MAX_CLAUSES`] keeps the tour: the completion greedy is
/// quadratic in the group.
fn order_group(
    group: &mut [usize],
    formula: &CnfFormula,
    finishes_here: impl Fn(VarId) -> bool,
    scratch: &mut OrderScratch,
) {
    let sets = DenseClauseVars::new(group, formula);
    let tour = proximity_order(&sets);
    let order = if group.len() > COMPLETION_ORDER_MAX_CLAUSES {
        tour
    } else {
        scratch.load(&sets, finishes_here);
        let completion = scratch.completion_order(&sets);
        let any_finishes = scratch.finishes.contains(&true);
        let take = prefer_completion(
            scratch.profile(&sets, &completion),
            scratch.profile(&sets, &tour),
            any_finishes,
        );
        if take { completion } else { tour }
    };

    // Apply `order` to `group` through a reordered copy.
    let reordered: Vec<usize> = order.iter().map(|&k| group[k]).collect();
    group.copy_from_slice(&reordered);
}

/// The open profile of a clause order at a node. A variable is *open* once a
/// placed clause mentions it, until it *finishes*: its last clause in the
/// group is placed and the node is its last scope. A variable whose last
/// scope is higher stays open to the end of the order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OpenProfile {
    /// The largest open count after a clause of the order.
    max: usize,
    /// The open counts after each clause, summed over the order.
    sum: usize,
}

/// Whether the completion order replaces the proximity tour, given the two
/// orders' open profiles and whether any variable of the group finishes at
/// the node.
///
/// Where some variable finishes, the completion order must be clearly
/// smaller: its max at most [`COMPLETION_ORDER_MAX_RATIO`] of the tour's and
/// its sum at most [`COMPLETION_ORDER_SUM_RATIO`] of the tour's. Where none
/// does, both orders end with every variable open, so the maxima tie and only
/// the order of touching differs; a smaller sum is enough. An equal sum keeps
/// the tour either way.
fn prefer_completion(completion: OpenProfile, tour: OpenProfile, any_finishes: bool) -> bool {
    let within = |c: usize, t: usize, ratio: f64| c as f64 <= ratio * t as f64;
    let clearly_smaller = within(completion.max, tour.max, COMPLETION_ORDER_MAX_RATIO)
        && within(completion.sum, tour.sum, COMPLETION_ORDER_SUM_RATIO);
    completion.sum < tour.sum && (clearly_smaller || !any_finishes)
}

/// What placing a clause next does through one of its variables, from the
/// variable's state: whether it is open, how many unplaced clauses mention it
/// (this one included), and whether the node is its last scope. The pair is
/// (the change in the open count, minus the open variables it finishes):
/// placing the clause opens an unopened variable unless this is its last
/// clause and it finishes at once, and finishes an open one on its last
/// clause.
#[inline]
fn effect(open: bool, pending: u32, finishes: bool) -> (i32, i32) {
    match (open, finishes && pending == 1) {
        (false, false) => (1, 0),
        (true, true) => (-1, -1),
        _ => (0, 0),
    }
}

/// The completion order's and the open profiles' buffers over one group, in
/// its dense variable ids and group positions. One value serves every group
/// of a compile ([`OrderScratch::load`] refills it), so a group allocates only
/// when it outgrows every group before it.
#[derive(Default)]
struct OrderScratch {
    /// Per variable: the node is its last scope, so it finishes here.
    finishes: Vec<bool>,
    /// The group positions of the clauses mentioning variable v are
    /// `occurrences[occurrence_starts[v]..occurrence_starts[v + 1]]`.
    occurrence_starts: Vec<u32>,
    occurrences: Vec<u32>,
    /// Per variable: the clauses mentioning it not yet placed.
    pending: Vec<u32>,
    /// Per variable: open (see [`OpenProfile`]).
    open: Vec<bool>,
    /// Per group position: the clause's rank as the next placement, the sum
    /// of [`effect`] over its variables; lower is better.
    rank: Vec<(i32, i32)>,
    /// The completion greedy's unplaced, non-pendant group positions,
    /// increasing.
    unplaced: Vec<u32>,
}

impl OrderScratch {
    /// Take up the group `sets`: which variables finish at its node, and each
    /// variable's clauses.
    fn load(&mut self, sets: &DenseClauseVars, finishes_here: impl Fn(VarId) -> bool) {
        let num_vars = sets.ids.len();
        self.finishes.clear();
        self.finishes.extend(sets.ids.iter().map(|&var| finishes_here(var)));

        self.occurrence_starts.clear();
        self.occurrence_starts.resize(num_vars + 1, 0);
        for &v in &sets.vars {
            self.occurrence_starts[v as usize + 1] += 1;
        }
        for v in 0..num_vars {
            self.occurrence_starts[v + 1] += self.occurrence_starts[v];
        }
        // `pending` serves as the fill cursor here; every user resets it.
        self.pending.clear();
        self.pending.extend_from_slice(&self.occurrence_starts[..num_vars]);
        self.occurrences.clear();
        self.occurrences.resize(sets.vars.len(), 0);
        for k in 0..sets.sizes.len() {
            for &v in sets.vars_of(k) {
                let cursor = &mut self.pending[v as usize];
                self.occurrences[*cursor as usize] = k as u32;
                *cursor += 1;
            }
        }
    }

    /// Set every variable's pending count to its clause count.
    fn reset_pending(&mut self) {
        self.pending.clear();
        self.pending.extend(self.occurrence_starts.windows(2).map(|w| w[1] - w[0]));
    }

    /// The open profile of `order`, a permutation of the group's positions.
    fn profile(&mut self, sets: &DenseClauseVars, order: &[usize]) -> OpenProfile {
        self.reset_pending();
        let mut open = 0usize;
        let mut profile = OpenProfile::default();
        for &k in order {
            for &v in sets.vars_of(k) {
                let v = v as usize;
                if self.pending[v] == self.occurrence_starts[v + 1] - self.occurrence_starts[v] {
                    open += 1;
                }
                self.pending[v] -= 1;
                if self.pending[v] == 0 && self.finishes[v] {
                    open -= 1;
                }
            }
            profile.max = profile.max.max(open);
            profile.sum += open;
        }
        profile
    }

    /// The completion order of the group: a greedy that places next the
    /// clause leaving the fewest open variables, ties to the clause finishing
    /// more open variables, then to the earlier group position, with the
    /// pendant clauses last. `order[k]` is the group position placed k-th.
    ///
    /// A *pendant* clause is one none of whose variables occurs in another
    /// clause of the group and all of whose variables finish at the node.
    /// The open profile scores a pendant free wherever it goes, since it
    /// opens and finishes its variables at once, so the greedy would place
    /// every pendant first. But a pendant crosses the node: it couples the
    /// node's two sides and multiplies the accumulator until its variables
    /// are summed out, and every clause after it folds into that larger
    /// diagram. So the pendants are held back and appended, in group order,
    /// after every other clause.
    ///
    /// Each clause's rank is kept current as clauses are placed: a placement
    /// changes the state of its own variables only, and a variable's
    /// [`effect`] changes at most twice while it has unplaced clauses (when it
    /// opens, and when one clause is left), so the updates cost a few passes
    /// over the group's literals. What stays quadratic is the scan for the
    /// best rank at each placement.
    fn completion_order(&mut self, sets: &DenseClauseVars) -> Vec<usize> {
        let n = sets.sizes.len();
        self.reset_pending();
        self.open.clear();
        self.open.resize(sets.ids.len(), false);

        let mut pendants = Vec::new();
        self.unplaced.clear();
        for k in 0..n {
            let pendant = sets.vars_of(k).iter().all(|&v| self.pending[v as usize] == 1 && self.finishes[v as usize]);
            if pendant {
                pendants.push(k);
            } else {
                self.unplaced.push(k as u32);
            }
        }

        // Nothing is open yet.
        self.rank.clear();
        for k in 0..n {
            let rank = sets.vars_of(k).iter().fold((0, 0), |(opened, finished), &v| {
                let (d_open, d_finished) = effect(false, self.pending[v as usize], self.finishes[v as usize]);
                (opened + d_open, finished + d_finished)
            });
            self.rank.push(rank);
        }

        let mut order = Vec::with_capacity(n);
        while !self.unplaced.is_empty() {
            // The lowest rank; a tie keeps the earlier position.
            let mut best = 0;
            let mut best_rank = self.rank[self.unplaced[0] as usize];
            for (i, &k) in self.unplaced.iter().enumerate().skip(1) {
                let rank = self.rank[k as usize];
                if rank < best_rank {
                    best = i;
                    best_rank = rank;
                }
            }
            let k = self.unplaced.remove(best) as usize;
            self.place(sets, k);
            order.push(k);
        }
        order.extend(pendants);
        order
    }

    /// Place the clause at group position `k`: update its variables' state,
    /// and the rank of every clause whose [`effect`] through one of them
    /// changed.
    fn place(&mut self, sets: &DenseClauseVars, k: usize) {
        for &v in sets.vars_of(k) {
            let v = v as usize;
            let finishes = self.finishes[v];
            let before = effect(self.open[v], self.pending[v], finishes);
            self.pending[v] -= 1;
            self.open[v] = !finishes || self.pending[v] > 0;
            if self.pending[v] == 0 {
                continue;
            }
            let after = effect(self.open[v], self.pending[v], finishes);
            if after != before {
                let delta = (after.0 - before.0, after.1 - before.1);
                let start = self.occurrence_starts[v] as usize;
                let end = self.occurrence_starts[v + 1] as usize;
                // Placed clauses are updated too; they are never ranked again.
                for &j in &self.occurrences[start..end] {
                    let rank = &mut self.rank[j as usize];
                    rank.0 += delta.0;
                    rank.1 += delta.1;
                }
            }
        }
    }
}

/// The proximity tour of a clause group (greedy nearest-neighbor): clauses
/// that share variables are placed adjacent to improve compilation locality.
/// `order[k]` is the group position placed k-th.
///
/// The tour starts at the clause with the most distinct variables, then
/// repeatedly appends the unplaced clause sharing the most variables with the
/// last placed one. Ties go to the later clause in the group: the last of the
/// largest clauses starts, and the highest-indexed of the best successors
/// wins, a zero overlap included. Compilation depends on this exact order, and
/// a test pins it against a direct hash-set version.
///
/// A group over at most 256 variables keeps a bitset per clause and counts an
/// overlap as the popcount of the two bitsets' intersection; a wider one
/// keeps each clause's variable list and counts it against a marker array
/// stamped with the last clause. Each step scans the unplaced clauses from
/// the highest index down and stops at the first that holds every variable
/// of the last clause, which no lower index can beat.
fn proximity_order(sets: &DenseClauseVars) -> Vec<usize> {
    match sets.ids.len().div_ceil(64) {
        1 => proximity_tour(BitsetRows::<1>::new(sets), &sets.sizes),
        2 => proximity_tour(BitsetRows::<2>::new(sets), &sets.sizes),
        3 | 4 => proximity_tour(BitsetRows::<4>::new(sets), &sets.sizes),
        _ => proximity_tour(MarkedLists::new(sets), &sets.sizes),
    }
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

/// A clause group's variable sets over dense variable ids `0..ids.len()`.
struct DenseClauseVars {
    /// The formula variable of each dense id.
    ids: Vec<VarId>,
    /// Clause i's distinct variables, sorted, are `vars[starts[i]..starts[i + 1]]`.
    starts: Vec<usize>,
    vars: Vec<u32>,
    /// Clause i's distinct-variable count.
    sizes: Vec<u32>,
}

impl DenseClauseVars {
    fn new(clauses: &[usize], formula: &CnfFormula) -> Self {
        let mut dense: rustc_hash::FxHashMap<VarId, u32> = rustc_hash::FxHashMap::default();
        let mut ids = Vec::new();
        let mut starts = Vec::with_capacity(clauses.len() + 1);
        let mut vars = Vec::new();
        let mut sizes = Vec::with_capacity(clauses.len());
        let mut scratch: Vec<u32> = Vec::new();
        starts.push(0);
        for &ci in clauses {
            scratch.clear();
            for lit in &formula.clauses()[ci].literals {
                let id = *dense.entry(lit.var).or_insert_with(|| {
                    ids.push(lit.var);
                    ids.len() as u32 - 1
                });
                scratch.push(id);
            }
            scratch.sort_unstable();
            scratch.dedup();
            vars.extend_from_slice(&scratch);
            starts.push(vars.len());
            sizes.push(scratch.len() as u32);
        }
        DenseClauseVars { ids, starts, vars, sizes }
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
        MarkedLists { sets, position: (0..n).collect(), mark: vec![0; sets.ids.len()], stamp: 0 }
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
