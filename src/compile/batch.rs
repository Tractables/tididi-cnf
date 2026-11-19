//! Folding one vtree node's clauses into its accumulator, a batch at a time.
//!
//! A node's clauses are not conjoined one by one into the accumulator: runs
//! of them, the batches, are each built into a diagram first, and the batch
//! is merged. `build_one_batch` decides where a batch stops (the floor and
//! its width cap, an extension past the floor that weighs the batch's build
//! cost against the merge's accumulator cost, UNSAT), `merge_batch` merges
//! one batch and says whether the accumulator went UNSAT and whether the
//! merge stayed absorbing, and `batch_accumulate` is the fold over the two.
//! The width caps and the spine marking that reads a batch's width and pairs
//! are here as well. `step.rs` decides which node is folded and when.

use std::collections::HashMap;
use std::sync::Arc;

use tididi::{Engine, Literal, OperationError};
use tididi::diagram::Tdd;
use tididi::vtree::{VarId, Vtree, VtreeIdx};

use crate::budget::deadline_passed;
use super::marginalize::{StepTargets, Summing, summable};
use super::minimize::{MinimizeGate, minimize_contract_only};
use super::reach::ReachScratch;
use super::schedule::{StepVars, step_completions, step_last_clauses};
use super::step::{DiagramClauses, PassState, StepContext};

/// Clauses per batch below which a batch never stops for its cost, the floor
/// ([`BatchStop::build_one_batch`]): every batch folds this many clauses unless the
/// node's clauses run out, the batch goes UNSAT or its width passes
/// [`BATCH_WIDTH_LIMIT`]. A batch that may not extend stops here. A fold that
/// starts from a large accumulator uses [`BATCH_LARGE_SIZE`] instead
/// ([`batch_floor`]).
const BATCH_SIZE: usize = 32;

/// The accumulator width up to which a batch grows with no width cap.
const UNCAPPED_ACC_WIDTH: usize = 64;

/// The width past which a batch below the [`BATCH_SIZE`] floor stops growing
/// once the accumulator is wider than [`UNCAPPED_ACC_WIDTH`], whether or not
/// it may extend.
const BATCH_WIDTH_LIMIT: usize = 256;

/// The width an extended batch never passes once the accumulator is wider
/// than [`UNCAPPED_ACC_WIDTH`]: a wide batch merged into a wide accumulator
/// multiplies the merge's unreduced product.
const BATCH_WIDTH_GUARD: usize = 1024;

/// How many times more one merge costs per accumulator pair than conjoining
/// one clause into a batch costs per batch pair; the extension past the floor
/// weighs the two ([`BatchStop::build_one_batch`]). A merge conjoins and reduces the
/// whole accumulator, where a batch conjoin touches only the batch.
const BATCH_MERGE_COST_RATIO: usize = 5;

/// A merge is absorbing when its unreduced product holds at most this
/// fraction (numerator, denominator) of the accumulator's pairs; only after
/// an absorbing merge may the next batch at the node extend past the floor
/// ([`next_batch_limit`]). A merge whose clauses the accumulator absorbs has
/// a product a little above the accumulator's pairs, and one whose clauses
/// multiply it a product several times as large; the factor sits between the
/// two, low enough to end the extension as a node starts to turn from
/// absorbing to expanding.
const BATCH_ABSORB_FACTOR: (usize, usize) = (3, 2);

/// After an absorbing merge the next batch may hold up to this fraction
/// (numerator, denominator) of the clauses of the batch just merged, counted
/// from at least the floor ([`next_batch_limit`]).
const BATCH_GROWTH: (usize, usize) = (3, 2);

/// Accumulator pairs above which a node's fold batches at the large floor
/// ([`BATCH_LARGE_SIZE`] clauses, width cap [`BATCH_LARGE_WIDTH_LIMIT`])
/// instead of [`BATCH_SIZE`] and [`BATCH_WIDTH_LIMIT`] ([`batch_floor`]).
/// Every merge into an accumulator this large costs time in proportion to its
/// pairs whether or not the node's clauses multiply it, and the earned
/// extension only lengthens batches after absorbing merges, so a node that
/// starts large and expands pays a full accumulator merge every floor's worth
/// of clauses. The large floor merges such a node less often, and leaves the
/// peak pairs as they are.
const BATCH_LARGE_ENTRY_PAIRS: usize = 10_000_000;

/// The floor's clause count for a fold that starts above
/// [`BATCH_LARGE_ENTRY_PAIRS`].
const BATCH_LARGE_SIZE: usize = 128;

/// The below-floor width cap for a fold that starts above
/// [`BATCH_LARGE_ENTRY_PAIRS`].
const BATCH_LARGE_WIDTH_LIMIT: usize = 1024;

/// Conjoin a node's clauses, `clauses`, into the accumulator a batch at a
/// time.
///
/// Within a batch, clauses are conjoined one by one with
/// [`Engine::and_clause`], each followed by twin contraction alone. Between
/// batches, the batch is merged into the accumulator and
/// [`MinimizeGate::adaptive_minimize`] picks the reduction to run.
///
/// Checks `ctx.job.deadline` before each batch and returns
/// `OperationError::Stopped` once it has passed; returns early once the
/// accumulator is UNSAT.
pub(crate) fn batch_accumulate(
    eng: &Engine,
    clauses: &[usize],
    ctx: &StepContext<'_>,
    vtree: &Arc<Vtree>,
    mut acc: Option<Tdd>,
    state: &mut PassState,
    marginalize_targets: Option<&StepTargets<'_>>,
) -> Result<Option<Tdd>, OperationError> {
    if clauses.is_empty() {
        return Ok(acc);
    }

    // For each of the node's clauses, the targets the schedule frees at this
    // step whose last mention it is: each is summed out at the merge of the
    // batch that holds that clause, rather than at the step's end.
    let mut streaming = marginalize_targets.map(|targets| {
        let clause_lits: Vec<&[Literal]> = clauses.iter().map(|&ci| ctx.clauses.get(ci)).collect();
        StreamTargets::new(&clause_lits, vtree, targets)
    });

    // `limit` is the most clauses the next batch may hold: the floor until a
    // merge at this node stays absorbing, then `next_batch_limit` of it. The
    // floor is fixed for the whole fold by the accumulator it starts from.
    let floor = batch_floor(acc.as_ref());
    state.minimize.start_fold(eng);
    let mut limit = floor.size;
    let mut i = 0;
    while i < clauses.len() {
        if deadline_passed(ctx.job.deadline) { return Err(OperationError::Stopped); }
        let start_clause = i;
        let mut stop = BatchStop::for_fold(vtree, clauses, floor, limit, acc.as_ref(), streaming.as_ref());
        stop.projections = ctx.job.run.projections;
        let (batch, next) = stop.build_one_batch(eng, ctx.clauses, start_clause, &mut state.on_spine)?;
        i = next;
        // Absorption decides whether the next batch may extend, so it is only
        // measured when enough clauses remain for the next batch to pass the
        // floor. It needs the accumulator's pairs, which the stop rule may
        // already have read.
        let acc_pairs = (clauses.len() - i > floor.size).then(|| stop.acc_pairs());
        // The targets this merge may sum out: a projecting compile keeps its
        // leaf levels Boolean, which can leave a merge that completes targets
        // with none to sum out.
        let kept;
        let merge_targets = match streaming.as_mut().and_then(|s| s.completed_in(start_clause, i)) {
            Some(Completed::Nodes(targets)) => {
                kept = summable(vtree, targets, ctx.job.output.forget().is_some());
                Some(Summing::Nodes(&kept))
            }
            Some(Completed::Vars(summing)) => Some(summing),
            None => None,
        };
        let merged =
            merge_batch(eng, batch, &mut acc, &mut state.minimize, merge_targets, acc_pairs, i == clauses.len())?;
        if merged.unsat {
            return Ok(acc);
        }
        limit = next_batch_limit(i - start_clause, merged.absorbed, floor.size);
    }
    Ok(acc)
}

/// The floor one node's fold batches at: the clauses every batch folds before
/// it may stop for its cost, and the width cap below them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BatchFloor {
    /// The clauses every batch folds before it may stop for its cost. The
    /// cofactor step's guard (`compile::cofactor`) counts a node's merges at
    /// one per this many of its clauses.
    pub(super) size: usize,
    width_limit: usize,
}

/// The floor for a fold that starts from `acc`: [`BATCH_SIZE`] clauses under
/// [`BATCH_WIDTH_LIMIT`], or [`BATCH_LARGE_SIZE`] under
/// [`BATCH_LARGE_WIDTH_LIMIT`] once the accumulator holds more than
/// [`BATCH_LARGE_ENTRY_PAIRS`] pairs. Fixed at the fold's start: a fold that
/// starts small and grows past the threshold keeps the small floor, so the
/// larger one only applies at nodes that begin with an accumulator whose
/// merges dominate. Read the library's pair total so later batch decisions
/// reuse the cached count, and implicit levels need no per-node scan.
pub(super) fn batch_floor(acc: Option<&Tdd>) -> BatchFloor {
    if acc.is_some_and(|a| a.pair_count() > BATCH_LARGE_ENTRY_PAIRS) {
        BatchFloor { size: BATCH_LARGE_SIZE, width_limit: BATCH_LARGE_WIDTH_LIMIT }
    } else {
        BatchFloor { size: BATCH_SIZE, width_limit: BATCH_WIDTH_LIMIT }
    }
}

/// The most clauses the batch after one of `merged` clauses may hold, at a
/// fold whose floor is `floor` clauses.
///
/// An absorbing merge ([`merge_batch_into_acc`]: its unreduced product stayed
/// within [`BATCH_ABSORB_FACTOR`] times the accumulator's pairs) lets the
/// next batch extend past the floor to [`BATCH_GROWTH`] times the batch just
/// merged, counted from at least the floor; any other merge, and the
/// merge-less first batch, leave the next batch at the floor. A node's
/// batches thus grow geometrically while its merges stay absorbing, and the
/// first merge that multiplies the accumulator sends the node back to
/// floor-sized batches. The growth factor bounds how far one batch can run
/// past the evidence: a node's clauses can turn from absorbing to expanding
/// between two clauses, and one long batch merged across that turn
/// multiplies the unreduced product far beyond what the same clauses cost in
/// floor-sized merges.
fn next_batch_limit(merged: usize, absorbed: bool, floor: usize) -> usize {
    if absorbed { merged.max(floor) * BATCH_GROWTH.0 / BATCH_GROWTH.1 } else { floor }
}

/// The marginalize targets that become final within each batch of one
/// node's fold.
enum StreamTargets<'a> {
    /// On a bare root.
    Nodes {
        /// Per clause position, the sub-vtrees whose last mention it is,
        /// children before parents.
        completions: Vec<Vec<VtreeIdx>>,
        /// What `completed_in` last returned.
        set: Vec<VtreeIdx>,
    },
    /// On a home.
    Vars {
        step: StepVars<'a>,
        /// The variables the step completes, each with its last clause
        /// position ([`step_last_clauses`]).
        last: HashMap<VarId, u32>,
        /// Per clause position, whether it is some variable's last mention.
        completes: Vec<bool>,
    },
}

/// What [`StreamTargets::completed_in`] found.
enum Completed<'s> {
    Nodes(&'s [VtreeIdx]),
    Vars(Summing<'s>),
}

impl<'a> StreamTargets<'a> {
    fn new(clause_lits: &[&[Literal]], vtree: &Vtree, step_targets: &StepTargets<'a>) -> Self {
        match step_targets {
            StepTargets::Nodes(targets) => StreamTargets::Nodes {
                completions: step_completions(clause_lits, vtree, targets),
                set: Vec::new(),
            },
            &StepTargets::Vars(step) => {
                let last = step_last_clauses(clause_lits, step);
                let mut completes = vec![false; clause_lits.len()];
                for &at in last.values() {
                    completes[at as usize] = true;
                }
                StreamTargets::Vars { step, last, completes }
            }
        }
    }

    /// Whether the clause at position `p` is the last mention of some target.
    fn completes(&self, p: usize) -> bool {
        match self {
            StreamTargets::Nodes { completions, .. } => !completions[p].is_empty(),
            StreamTargets::Vars { completes, .. } => completes[p],
        }
    }

    /// The targets whose last mention falls in clause positions
    /// `start..end`, or `None` when there are none. No later clause mentions
    /// them, so they can be marginalized at this merge.
    ///
    /// On a bare root each sub-vtree has one last mention, so none is listed
    /// twice, and a node's last mention is never before a descendant's, so
    /// reading the positions in order keeps every child before its parent.
    fn completed_in(&mut self, start: usize, end: usize) -> Option<Completed<'_>> {
        match self {
            StreamTargets::Nodes { completions, set } => {
                set.clear();
                for subs in &completions[start..end] {
                    set.extend_from_slice(subs);
                }
                if set.is_empty() { None } else { Some(Completed::Nodes(set)) }
            }
            StreamTargets::Vars { step, last, completes } => completes[start..end].contains(&true).then(|| {
                Completed::Vars(Summing::Vars { step: *step, last: Some(last), through: end as u32 - 1 })
            }),
        }
    }
}

/// One batch built by [`BatchStop::build_one_batch`]: a batch diagram (boxed,
/// since a `Tdd` is much larger than a clause slice), or a single clause
/// conjoined straight into a live accumulator.
enum BuiltBatch<'a> {
    Diagram(Box<Tdd>),
    SingleClause(&'a [Literal]),
}

/// What one merge told the fold.
struct Merged {
    /// The merged accumulator is UNSAT, and stays so whatever clauses remain,
    /// so the fold can stop early.
    unsat: bool,
    /// The merge was absorbing ([`merge_batch_into_acc`]); false when not
    /// measured.
    absorbed: bool,
}

/// Merge one built `batch` into `acc`, measuring absorption against
/// `acc_pairs`, the accumulator's pair count, when it is given. An apply
/// error aborts the pass.
///
/// A [`BuiltBatch::SingleClause`] is conjoined directly into `acc`. That
/// happens only when no further clause can follow in the batch, so no
/// absorption is measured. `fold_end` marks the fold's last merge, as
/// [`MinimizeGate::adaptive_minimize`] takes it.
fn merge_batch(
    eng: &Engine,
    batch: BuiltBatch<'_>,
    acc: &mut Option<Tdd>,
    state: &mut MinimizeGate,
    merge_targets: Option<Summing<'_>>,
    acc_pairs: Option<usize>,
    fold_end: bool,
) -> Result<Merged, OperationError> {
    let acc_in = acc.take();
    let (merged, absorbed) = match batch {
        BuiltBatch::Diagram(batch) => merge_batch_into_acc(eng, acc_in, *batch, state, merge_targets, acc_pairs, fold_end)?,
        BuiltBatch::SingleClause(clause) => (
            merge_clause_into_acc(
                eng,
                acc_in.expect("a one-clause batch builds no diagram only for a live accumulator"),
                clause,
                state,
                merge_targets,
                fold_end,
            )?,
            false,
        ),
    };
    let unsat = !eng.is_sat(&merged)?;
    *acc = Some(merged);
    Ok(Merged { unsat, absorbed })
}

/// Accumulate the ancestor paths of a batch's variables, recording only new marks.
/// Variables have already been validated against the compilation vtree.
#[inline]
fn mark_clause_levels(
    vtree: &Vtree,
    clause: &[Literal],
    visited: &mut [bool],
    newly_marked: &mut Vec<VtreeIdx>,
) {
    for literal in clause {
        let mut level = vtree.leaf_of(literal.var).expect("the vtree carries this variable");
        loop {
            if visited[level.idx()] { break; }
            visited[level.idx()] = true;
            newly_marked.push(level);
            match vtree.node(level).parent() {
                Some(parent) => level = parent,
                None => break,
            }
        }
    }
}

/// The arena width of a batch diagram under construction on the union of the
/// folded clauses' spines (`spine`): its `Tdd::max_width`, which counts the
/// nodes an operation left unnamed as well, read without sweeping every vtree
/// level. It bounds the reached width ([`ReachScratch::width_on_spine`]) from
/// above, so a cap it does not pass needs no reachability walk.
///
/// Why the spine suffices. A batch starts as `Tdd::clause(first clause)`, which
/// gives every internal level outside that clause's spine a single One node
/// and every leaf level none; [`Engine::and_clause`] rebuilds only the paths
/// from the clause's variables to the root, and `minimize_contract_only` only
/// merges twins, so a level off the running spine union keeps width at most 1.
/// The root is on every spine and holds a live node once the batch is
/// satisfiable, which the caller tests first.
fn batch_max_width(local: &Tdd, spine: &[VtreeIdx]) -> usize {
    spine.iter().map(|&t| local.level(t).slot_count()).max().unwrap_or(0)
}

/// The batch diagram's arena pairs on the spine union `spine`: its
/// `pair_count` less the one trivial pair of each internal level off the union
/// (off-spine levels never grow, [`batch_max_width`]). The extension rule reads
/// it as the cost of conjoining one more clause. Unlike the width it counts
/// the nodes an operation left unnamed: a reached count needs a walk over
/// every reached pair at every check, which costs Boolean compiles up to 8% of
/// their instructions, while this sum reads one count per node.
fn batch_spine_pairs(local: &Tdd, spine: &[VtreeIdx]) -> usize {
    spine
        .iter()
        .map(|&t| {
            let level = local.level(t);
            (0..level.nodes().len()).map(|n| level.pair_count_at(n)).sum::<usize>()
        })
        .sum()
}

/// Where one batch may stop, fixed before it is built.
struct BatchStop<'a> {
    /// The vtree the batch is built on.
    vtree: &'a Arc<Vtree>,
    /// The node's clause indices in the order the fold visits them.
    clauses: &'a [usize],
    /// The width that closes any batch below `floor` clauses
    /// ([`batch_width_cap`] at the fold's [`BatchFloor`] width limit).
    width_cap: usize,
    /// The width no batch passes once past `floor` clauses
    /// ([`batch_width_cap`] at [`BATCH_WIDTH_GUARD`]).
    guard: usize,
    /// The fold's floor ([`batch_floor`]): the clauses a batch folds before
    /// it may stop for its cost.
    floor: usize,
    /// The most clauses the batch may hold ([`next_batch_limit`]); a batch
    /// may extend past the floor only when this exceeds `floor`.
    limit: usize,
    /// The accumulator the batch will be merged into.
    acc: Option<&'a Tdd>,
    /// The node's marginalize targets by clause position, when it streams.
    streaming: Option<&'a StreamTargets<'a>>,
    /// The batch starts on the projection of `vtree` on its first clause's
    /// variables ([`super::CompileRun::projections`]); its width and pairs
    /// are then read off the whole diagram.
    projections: bool,
}

impl<'a> BatchStop<'a> {
    fn for_fold(
        vtree: &'a Arc<Vtree>,
        clauses: &'a [usize],
        floor: BatchFloor,
        limit: usize,
        acc: Option<&'a Tdd>,
        streaming: Option<&'a StreamTargets<'a>>,
    ) -> Self {
        let acc_width = acc.map_or(0, Tdd::max_width);
        Self {
            vtree,
            clauses,
            width_cap: batch_width_cap(acc_width, floor.width_limit),
            guard: batch_width_cap(acc_width, BATCH_WIDTH_GUARD),
            floor: floor.size,
            limit,
            acc,
            streaming,
            projections: false,
        }
    }

    fn completes(&self, p: usize) -> bool {
        self.streaming.is_some_and(|s| s.completes(p))
    }

    /// The accumulator's pair count (0 before the first merge), cached by
    /// the diagram and shared by extension and absorption checks.
    fn acc_pairs(&self) -> usize {
        self.acc.map_or(0, Tdd::pair_count)
    }

    /// Build one batch by conjoining clauses one by one from
    /// `self.clauses[start]`. Each conjoin is followed by twin contraction,
    /// which keeps the width and pair count the stop checks read honest.
    /// `on_spine` is the pass's spine-marking scratch.
    ///
    /// The batch stops on UNSAT, at `self.limit` clauses, and otherwise where
    /// its floor and its extension say:
    ///
    /// - **Floor.** Every batch keeps folding until it holds `self.floor`
    ///   clauses or its width exceeds `self.width_cap`, so the merge with a
    ///   wide accumulator that follows stays cheap. A batch the cap stops
    ///   stops even when it may extend: a batch that turns wide before the
    ///   floor is not behaving like the absorbed batches that earned the
    ///   extension, and merging it wide into a wide accumulator multiplies
    ///   the merge's product. A batch that may not extend
    ///   (`self.limit == self.floor`) stops at the floor.
    /// - **Extension.** Past the floor, a merge costs time in proportion to
    ///   the accumulator's pairs, nearly independent of the batch, while
    ///   conjoining one more clause costs time in proportion to the batch's
    ///   pairs. So the batch keeps folding while the next clause's build cost
    ///   is below the running per-clause average of the batch's whole cost,
    ///   its build so far plus its merge: with `P_j` the batch's pairs once
    ///   it holds `j` clauses (the cost of conjoining clause `j + 1`), `k`
    ///   clauses folded and `A` the accumulator's pairs, it continues while
    ///   `k * P_k < P_1 + ... + P_{k-1} + BATCH_MERGE_COST_RATIO * A`, the
    ///   marginal-below-average rule in units of one batch-pair conjoin. It
    ///   also stops once its width exceeds `self.guard`, which keeps a wide
    ///   batch from meeting a wide accumulator in one product, and after a
    ///   clause that is a marginalize target's last mention, so an extension
    ///   never delays a marginalization past the boundary the floor would
    ///   have given it.
    ///
    /// The width is that of the nodes the batch's output reaches on the spine
    /// union ([`ReachScratch::width_on_spine`]), so that nodes the library
    /// leaves unnamed move no width stop; the walk runs only when the arena width
    /// ([`batch_max_width`]) passes the cap that applies. The pairs are the
    /// arena's on the spine union ([`batch_spine_pairs`]). The accumulator's
    /// pair count is its cached one ([`BatchStop::acc_pairs`]), which the
    /// extension decision and the merge's absorption check share.
    ///
    /// Returns the built batch and the index of the next clause, at least
    /// `start + 1`: the first clause is always taken. A one-clause batch
    /// merged into a live accumulator needs no diagram of its own, since
    /// `merge_clause_into_acc` conjoins the clause straight into the
    /// accumulator. So when the accumulator is live and the end of the clause
    /// list or `self.limit` ends the batch after its first clause, both known
    /// before any diagram exists, no diagram is built and the batch is
    /// [`BuiltBatch::SingleClause`]. UNSAT and the width caps are tested only
    /// once a second clause is in, so they never end a batch at one clause.
    fn build_one_batch<'c>(
        &self,
        eng: &Engine,
        diagram_clauses: &'c DiagramClauses,
        start: usize,
        on_spine: &mut Vec<bool>,
    ) -> Result<(BuiltBatch<'c>, usize), OperationError> {
        let vtree = self.vtree;
        let clauses = self.clauses;
        // Does the batch reach `clauses[i]`, given it already holds `batch_count`
        // clauses? The single statement of the batch's *content-independent* stop
        // conditions — evaluated once up front for the skip decision and once per
        // round by the fold below, never restated.
        let extends_past = |i: usize, batch_count: usize| i < clauses.len() && batch_count < self.limit;

        if self.acc.is_some() && !extends_past(start + 1, 1) {
            return Ok((BuiltBatch::SingleClause(diagram_clauses.get(clauses[start])), start + 1));
        }

        let first = diagram_clauses.get(clauses[start]);
        let mut local = match self.projections {
            true => eng.clause(&clause_vtree(vtree, first), first.iter().copied())?,
            false => eng.clause(vtree, first.iter().copied())?,
        };
        // The running union of the folded clauses' spines, from which the stop
        // checks read the batch's width and pairs instead of sweeping every
        // level (`batch_max_width`, `batch_spine_pairs`). Tracked only when a
        // width cap is on or the batch may extend. All false on entry: only
        // spine entries are ever set, and they are reset from `touched` before
        // returning. An error return skips the reset, but an error ends the
        // pass, whose state the scratch is part of.
        let may_extend = self.limit > self.floor;
        let track_spine = !self.projections && (self.width_cap != usize::MAX || may_extend);
        let mut reach = ReachScratch::default();
        let mut touched = Vec::new();
        if track_spine {
            if on_spine.len() < vtree.num_nodes() {
                on_spine.resize(vtree.num_nodes(), false);
            }
            mark_clause_levels(vtree, diagram_clauses.get(clauses[start]), on_spine, &mut touched);
        }
        let mut built = 0usize;
        let spine_pairs = |local: &Tdd, touched: &[VtreeIdx]| match self.projections {
            true => local.pair_count(),
            false => batch_spine_pairs(local, touched),
        };
        let mut pairs = if may_extend { spine_pairs(&local, &touched) } else { 0 };
        let mut holds_completion = self.completes(start);
        let mut i = start + 1;
        let mut batch_count = 1;
        while extends_past(i, batch_count) {
            if track_spine {
                mark_clause_levels(vtree, diagram_clauses.get(clauses[i]), on_spine, &mut touched);
            }
            local = eng.and_clause(local, diagram_clauses.get(clauses[i]))?;
            minimize_contract_only(eng, &mut local)?;
            holds_completion |= self.completes(i);
            i += 1;
            batch_count += 1;
            if !eng.is_sat(&local)? { break; }
            let mut width = 0;
            if track_spine {
                width = batch_max_width(&local, &touched);
            } else if self.projections && self.width_cap != usize::MAX {
                width = local.max_width();
            }
            let cap = if batch_count < self.floor { self.width_cap } else { self.guard };
            if width > cap {
                width = match self.projections {
                    true => reach.width_on_spine(&local, local.vtree().internal_bottomup_slice()),
                    false => reach.width_on_spine(&local, &touched),
                };
            }
            if may_extend {
                built += pairs;
                pairs = spine_pairs(&local, &touched);
            }
            if self.stops_after(batch_count, width, holds_completion, built, pairs) {
                break;
            }
        }
        for &t in &touched { on_spine[t.idx()] = false; }
        Ok((BuiltBatch::Diagram(Box::new(local)), i))
    }

    /// Whether the batch should stop after folding `batch_count` clauses: below
    /// `self.floor`, `self.width_cap` stops the batch; at or above `self.floor`,
    /// the batch stops unless `self.limit > self.floor`, no marginalize target
    /// has completed, `width <= self.guard`, and the next step's amortized build
    /// cost stays below the merge cost.
    fn stops_after(
        &self,
        batch_count: usize,
        width: usize,
        holds_completion: bool,
        built: usize,
        pairs: usize,
    ) -> bool {
        if batch_count < self.floor {
            return width > self.width_cap;
        }
        if self.limit <= self.floor || holds_completion || width > self.guard {
            return true;
        }
        batch_count.saturating_mul(pairs)
            >= built.saturating_add(BATCH_MERGE_COST_RATIO.saturating_mul(self.acc_pairs()))
    }
}

/// The projection of `vtree` on `clause`'s variables.
pub(super) fn clause_vtree(vtree: &Arc<Vtree>, clause: &[Literal]) -> Arc<Vtree> {
    Vtree::projection(&tididi::vtree::Home::new(Arc::clone(vtree)), clause.iter().map(|lit| lit.var)).expect("the vtree has a leaf for every clause variable")
}

/// Maximum allowed batch-TDD width before the batch should be flushed into the
/// accumulator: no cap (`usize::MAX`) while the accumulator is at most
/// [`UNCAPPED_ACC_WIDTH`] wide, `limit` above that: the fold's floor width
/// limit ([`BATCH_WIDTH_LIMIT`] or [`BATCH_LARGE_WIDTH_LIMIT`],
/// [`batch_floor`]) below the floor, [`BATCH_WIDTH_GUARD`] for an extended
/// batch.
///
/// A wide batch merged into a wide accumulator is the conjunction's costliest
/// case, so once the accumulator is wide its batches are kept narrow, though
/// not so narrow that a batch shrinks to a single clause.
fn batch_width_cap(acc_width: usize, limit: usize) -> usize {
    if acc_width <= UNCAPPED_ACC_WIDTH { usize::MAX } else { limit }
}

/// Merge a batch TDD into the accumulator, marginalizing `marginalize_targets`
/// inside the conjunction, then let [`MinimizeGate::adaptive_minimize`] pick
/// the minimize to run. An apply error aborts the pass.
///
/// Also returns whether the merge was absorbing, its unreduced product
/// holding at most [`BATCH_ABSORB_FACTOR`] times the accumulator's pairs
/// `acc_pairs`, when they are given (false otherwise, and for the first
/// batch, which has nothing to merge into). Counting the product's pairs
/// reads every node once, a small fraction of the merge, which itself visits
/// every accumulator pair.
fn merge_batch_into_acc(
    eng: &Engine,
    acc: Option<Tdd>,
    batch: Tdd,
    state: &mut MinimizeGate,
    marginalize_targets: Option<Summing<'_>>,
    acc_pairs: Option<usize>,
    fold_end: bool,
) -> Result<(Tdd, bool), OperationError> {
    let Some(a) = acc else {
        // First batch: the batch builder's output is already minimized. If targets
        // are set (single-batch step), marginalize inline so streamed levels
        // reach their final state, there being no conjunction to stream them
        // through.
        let mut result = batch;
        if let Some(targets) = marginalize_targets {
            targets.sum_out(eng, &mut result)?;
        }
        return Ok((result, false));
    };

    let mut result = match marginalize_targets {
        Some(targets) => targets.and(eng, a, batch)?,
        None => eng.and(a, batch)?,
    };
    let (num, den) = BATCH_ABSORB_FACTOR;
    let absorbed = acc_pairs.is_some_and(|n| result.pair_count().saturating_mul(den) <= num.saturating_mul(n));

    let summed_out = marginalize_targets.is_some_and(|t| !t.is_empty());
    state.adaptive_minimize(eng, &mut result, fold_end, summed_out)?;

    Ok((result, absorbed))
}

/// Merge a one-clause batch: conjoin `clause` into `acc` with
/// [`Engine::and_clause`], let [`MinimizeGate::adaptive_minimize`] reduce,
/// then marginalize `marginalize_targets` (a batch merge marginalizes inside
/// its conjunction, before reducing).
fn merge_clause_into_acc(
    eng: &Engine,
    acc: Tdd,
    clause: &[Literal],
    state: &mut MinimizeGate,
    marginalize_targets: Option<Summing<'_>>,
    fold_end: bool,
) -> Result<Tdd, OperationError> {
    let mut result = eng.and_clause(acc, clause)?;
    state.adaptive_minimize(eng, &mut result, fold_end, false)?;
    if let Some(targets) = marginalize_targets {
        targets.sum_out(eng, &mut result)?;
    }
    Ok(result)
}

#[cfg(test)]
#[path = "tests/batch.rs"]
mod tests;
