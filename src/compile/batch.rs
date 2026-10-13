//! Folding one vtree node's clauses into its accumulator, a batch at a time.
//!
//! A node's clauses are not conjoined one by one into the accumulator: runs
//! of them, the batches, are each built into a diagram first, and the batch
//! is merged. `build_one_batch` decides where a batch stops (the floor and
//! its width cap, an extension past the floor that
//! weighs the batch's build cost against the merge's accumulator cost,
//! UNSAT), `merge_batch` merges one batch and says whether the accumulator
//! went UNSAT and whether the merge stayed absorbing, and `batch_accumulate`
//! is the fold over the two. The width caps and the spine marking that reads
//! a batch's width and pairs are here as well. `step.rs` decides which node
//! is folded and when.

use std::cell::OnceCell;
use std::sync::Arc;

use tididi::{Engine, Literal, OperationError};
use tididi::diagram::Tdd;
use tididi::vtree::{Vtree, VtreeIdx};

use crate::budget::deadline_passed;
use super::marginalize::summable;
use super::minimize::{MinimizeGate, minimize_contract_only};
use super::schedule::step_completions;
use super::step::{DiagramClauses, PassState, StepContext};

/// Clauses per batch below which a batch never stops for its cost, the floor
/// ([`build_one_batch`]): every batch folds this many clauses unless the
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
/// than [`UNCAPPED_ACC_WIDTH`]. A wider extended batch was seen to multiply
/// the merge's unreduced product more than a hundredfold.
const BATCH_WIDTH_GUARD: usize = 1024;

/// How many times more one merge costs per accumulator pair than conjoining
/// one clause into a batch costs per batch pair; the extension past the floor
/// weighs the two ([`build_one_batch`]). On formulas with few variables and
/// many clauses a merge, conjoin and reduction, measured about 150-180 ns per
/// accumulator pair, and a batch conjoin 20-35 ns per batch pair.
const BATCH_MERGE_COST_RATIO: usize = 5;

/// A merge is absorbing when its unreduced product holds at most this
/// fraction (numerator, denominator) of the accumulator's pairs; only after
/// an absorbing merge may the next batch at the node extend past the floor
/// ([`next_batch_limit`]). The merges that earned an extension measured
/// products 1.2-1.4 times the accumulator; where a node's clauses multiply
/// the diagram it is 3-20 times, and a node about to turn from absorbing to
/// expanding was seen at 1.75.
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
/// of clauses. On formulas whose late nodes start from accumulators of tens
/// of millions of pairs, the large floor cut compile time by a fifth to a
/// quarter at unchanged peak pairs.
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
    marginalize_targets: Option<&[VtreeIdx]>,
) -> Result<Option<Tdd>, OperationError> {
    if clauses.is_empty() {
        return Ok(acc);
    }

    // For each of the node's clauses, the targets the schedule frees at this
    // step whose last mention it is: each is summed out at the merge of the
    // batch that holds that clause, rather than at the step's end.
    let mut streaming = marginalize_targets.map(|targets| {
        let clause_lits: Vec<&[Literal]> =
            clauses.iter().map(|&ci| ctx.clauses.get(ci)).collect();
        StreamTargets {
            completions: step_completions(&clause_lits, vtree, targets),
            set: Vec::new(),
        }
    });

    // `limit` is the most clauses the next batch may hold: the floor until a
    // merge at this node stays absorbing, then `next_batch_limit` of it. The
    // floor is fixed for the whole fold by the accumulator it starts from.
    let floor = batch_floor(acc.as_ref());
    let mut limit = floor.size;
    let mut i = 0;
    while i < clauses.len() {
        if deadline_passed(ctx.job.deadline) { return Err(OperationError::Stopped); }
        let start_clause = i;
        let acc_width = acc.as_ref().map_or(0, Tdd::max_width);
        let stop = BatchStop {
            width_cap: batch_width_cap(acc_width, floor.width_limit),
            guard: batch_width_cap(acc_width, BATCH_WIDTH_GUARD),
            floor: floor.size,
            limit,
            acc: acc.as_ref(),
            acc_pairs: OnceCell::new(),
            streaming: streaming.as_ref(),
        };
        let mut local;
        (local, i) = build_one_batch(eng, ctx.clauses, vtree, clauses, start_clause, &stop, &mut state.on_spine)?;
        // Absorption decides whether the next batch may extend, so it is only
        // measured when enough clauses remain for the next batch to pass the
        // floor. It needs the accumulator's pairs, which the stop rule may
        // already have read.
        let acc_pairs = (clauses.len() - i > floor.size).then(|| stop.acc_pairs());
        // The first batch becomes the accumulator when there is none yet: a
        // diagram the pass creates.
        if acc.is_none() {
            local = local.map(|l| ctx.job.output.with_store(l));
        }
        // The targets this merge may sum out: a projecting compile keeps its
        // leaf levels Boolean, which can leave a merge that completes targets
        // with none to sum out.
        let merge_targets = streaming
            .as_mut()
            .and_then(|s| s.completed_in(start_clause, i))
            .map(|targets| summable(vtree, targets, ctx.job.output.forget().is_some()));
        let first_clause = ctx.clauses.get(clauses[start_clause]);
        let merged =
            merge_batch(eng, local, first_clause, &mut acc, &mut state.minimize, merge_targets.as_deref(), acc_pairs)?;
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
struct BatchFloor {
    size: usize,
    width_limit: usize,
}

/// The floor for a fold that starts from `acc`: [`BATCH_SIZE`] clauses under
/// [`BATCH_WIDTH_LIMIT`], or [`BATCH_LARGE_SIZE`] under
/// [`BATCH_LARGE_WIDTH_LIMIT`] once the accumulator holds more than
/// [`BATCH_LARGE_ENTRY_PAIRS`] pairs. Fixed at the fold's start: a fold that
/// starts small and grows past the threshold keeps the small floor, so the
/// larger one only applies at nodes that begin with an accumulator whose
/// merges dominate. The pair count stops once the threshold is passed.
fn batch_floor(acc: Option<&Tdd>) -> BatchFloor {
    if acc.is_some_and(|a| !a.pair_count_at_most(BATCH_LARGE_ENTRY_PAIRS)) {
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
struct StreamTargets {
    /// Per clause position, the sub-vtrees whose last mention it is, children
    /// before parents.
    completions: Vec<Vec<VtreeIdx>>,
    /// What `completed_in` last returned.
    set: Vec<VtreeIdx>,
}

impl StreamTargets {
    /// Whether the clause at position `p` is the last mention of some target.
    fn completes(&self, p: usize) -> bool {
        !self.completions[p].is_empty()
    }

    /// The sub-vtrees whose last mention falls in clause positions
    /// `start..end`, children before parents, or `None` when there are none.
    /// No later clause mentions them, so they can be marginalized at this
    /// merge.
    ///
    /// Each sub-vtree has one last mention, so none is listed twice, and a
    /// node's last mention is never before a descendant's, so reading the
    /// positions in order keeps every child before its parent.
    fn completed_in(&mut self, start: usize, end: usize) -> Option<&[VtreeIdx]> {
        self.set.clear();
        for subs in &self.completions[start..end] {
            self.set.extend_from_slice(subs);
        }
        if self.set.is_empty() { None } else { Some(&self.set) }
    }
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

/// Merge one built batch into `acc`, measuring absorption against
/// `acc_pairs`, the accumulator's pair count, when it is given. An apply
/// error aborts the pass.
///
/// `local` is `None` when [`build_one_batch`] built no diagram for a
/// one-clause batch; `first_clause` is then conjoined directly. That happens
/// only for the node's last clause, so no absorption is measured.
fn merge_batch(
    eng: &Engine,
    local: Option<Tdd>,
    first_clause: &[Literal],
    acc: &mut Option<Tdd>,
    state: &mut MinimizeGate,
    merge_targets: Option<&[VtreeIdx]>,
    acc_pairs: Option<usize>,
) -> Result<Merged, OperationError> {
    let acc_in = acc.take();
    let (merged, absorbed) = match local {
        Some(batch) => merge_batch_into_acc(eng, acc_in, batch, state, merge_targets, acc_pairs)?,
        None => (
            merge_clause_into_acc(
                eng,
                acc_in.expect("a one-clause batch builds no diagram only for a live accumulator"),
                first_clause,
                state,
                merge_targets,
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

/// `local.max_width()` for a batch diagram under construction, read off the
/// union of the folded clauses' spines (`spine`) instead of sweeping every
/// vtree level. Returns exactly what `Tdd::max_width` would.
///
/// Why it is exact. Three facts about a batch diagram, all local to
/// `build_one_batch`:
///
/// 1. **Off-spine levels start trivial.** The batch starts as
///    `Tdd::clause(first clause)`, which gives every internal level outside
///    that clause's spine a single One node (width 1) and every leaf level no
///    stored nodes at all (width 0).
/// 2. **Only spine levels ever grow.** [`Engine::and_clause`] rebuilds only
///    the paths from the clause's variables to the root and carries every
///    other level through; `minimize_contract_only` only merges twins, which
///    can shrink a level but never widen one. So a level outside the running spine union
///    still has width ≤ 1 at every check.
/// 3. **The root is always in the union.** A spine is the ancestor-closed
///    Steiner tree of the clause's leaves, so it contains the vtree root. At
///    the check point the diagram is satisfiable (the caller tested `is_sat`
///    first), so the root level holds at least one live node — hence the
///    spine-restricted maximum is ≥ 1 and dominates every off-spine level.
///
/// The single-leaf-vtree corner falls out: there is no internal level, both the
/// swept and the restricted maximum are 0.
fn batch_max_width(local: &Tdd, spine: &[VtreeIdx]) -> usize {
    spine.iter().map(|&t| local.level(t).slot_count()).max().unwrap_or(0)
}

/// The batch diagram's pairs on the spine union `spine`: its `pair_count`
/// less the one trivial pair of each internal level off the union (fact 1 of
/// [`batch_max_width`]; off-spine levels never grow). The extension rule
/// reads it as the cost of conjoining one more clause, which is linear in the
/// batch's pairs; the off-spine constant does not matter there, and skipping
/// those levels keeps the read proportional to the batch's spine.
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
    /// Its pair count, read at most once per batch ([`BatchStop::acc_pairs`]).
    acc_pairs: OnceCell<usize>,
    /// The node's marginalize targets by clause position, when it streams.
    streaming: Option<&'a StreamTargets>,
}

impl BatchStop<'_> {
    fn completes(&self, p: usize) -> bool {
        self.streaming.is_some_and(|s| s.completes(p))
    }

    /// The accumulator's pair count (0 before the first merge), read once and
    /// shared by the extension rule and the merge's absorption check.
    fn acc_pairs(&self) -> usize {
        *self.acc_pairs.get_or_init(|| self.acc.map_or(0, Tdd::pair_count))
    }
}

/// Build one batch diagram by conjoining clauses one by one from
/// `clauses[start]`. Each conjoin is followed by twin contraction, which keeps
/// the width and pair count the stop checks read honest. `on_spine` is the
/// pass's spine-marking scratch.
///
/// The batch stops on UNSAT, at `stop.limit` clauses, and otherwise where its
/// floor and its extension say:
///
/// - **Floor.** Every batch keeps folding until it holds `stop.floor` clauses
///   or its width exceeds `stop.width_cap`, so the merge with a wide
///   accumulator that follows stays cheap. A batch the cap stops stops even
///   when it may extend: a batch that turns wide before the floor is not
///   behaving like the absorbed batches that earned the extension, and
///   merging it wide into a wide accumulator was seen to multiply the merge's
///   product several times over. A batch that may not extend
///   (`stop.limit == stop.floor`) stops at the floor.
/// - **Extension.** Past the floor, a merge costs time in proportion to the
///   accumulator's pairs, nearly independent of the batch, while conjoining
///   one more clause costs time in proportion to the batch's pairs. So the
///   batch keeps folding while the next clause's build cost is below the
///   running per-clause average of the batch's whole cost, its build so far
///   plus its merge: with `P_j` the batch's pairs once it holds `j` clauses
///   (the cost of conjoining clause `j + 1`), `k` clauses folded and `A` the
///   accumulator's pairs, it continues while
///   `k * P_k < P_1 + ... + P_{k-1} + BATCH_MERGE_COST_RATIO * A`, the
///   marginal-below-average rule in units of one batch-pair conjoin. It also
///   stops once its width exceeds `stop.guard`, which keeps a wide batch from
///   meeting a wide accumulator in one product, and after a clause that is a
///   marginalize target's last mention, so an extension never delays a
///   marginalization past the boundary the floor would have given it.
///
/// The pairs are read off the spine union ([`batch_spine_pairs`]), and the
/// accumulator's pair count at most once per batch, at the first extension
/// decision ([`BatchStop::acc_pairs`], which the merge's absorption check
/// shares).
///
/// Returns the batch diagram and the index of the next clause, at least
/// `start + 1`: the first clause is always taken. A one-clause batch merged
/// into a live accumulator needs no diagram of its own, since
/// `merge_clause_into_acc` conjoins the clause straight into the
/// accumulator. So when the accumulator is live and the end of the clause
/// list or `stop.limit` ends the batch after its first clause, both known
/// before any diagram exists, no diagram is built and the batch is `None`.
/// UNSAT and the width caps are tested only once a second clause is in, so
/// they never end a batch at one clause.
fn build_one_batch(
    eng: &Engine,
    diagram_clauses: &DiagramClauses,
    vtree: &Arc<Vtree>,
    clauses: &[usize],
    start: usize,
    stop: &BatchStop<'_>,
    on_spine: &mut Vec<bool>,
) -> Result<(Option<Tdd>, usize), OperationError> {
    // Does the batch reach `clauses[i]`, given it already holds `batch_count`
    // clauses? The single statement of the batch's *content-independent* stop
    // conditions — evaluated once up front for the skip decision and once per
    // round by the fold below, never restated.
    let extends_past = |i: usize, batch_count: usize| i < clauses.len() && batch_count < stop.limit;

    if stop.acc.is_some() && !extends_past(start + 1, 1) {
        return Ok((None, start + 1));
    }

    let mut local = eng.clause(vtree, diagram_clauses.get(clauses[start]).iter().copied())?;
    let may_extend = stop.limit > stop.floor;
    // Spine bookkeeping: the running union of the folded clauses' spines, from
    // which the stop checks read the batch's max width and pairs instead of
    // sweeping every level (`batch_max_width`, `batch_spine_pairs`). Tracked
    // only when a width cap is on or the batch may extend.
    let track_spine = stop.width_cap != usize::MAX || may_extend;
    let mut touched: Vec<VtreeIdx> = Vec::new();
    // All false on entry: only spine entries are ever set, and they are reset
    // from `touched` before returning. An error return skips the reset, but
    // an error ends the pass, whose state the scratch is part of.
    if track_spine {
        if on_spine.len() < vtree.num_nodes() { on_spine.resize(vtree.num_nodes(), false); }
        mark_clause_levels(vtree, diagram_clauses.get(clauses[start]), on_spine, &mut touched);
    }
    // Extension state, kept only when the batch may extend: the build cost so
    // far (`P_1 + ... + P_{k-1}`), the current pairs `P_k`, and whether a
    // folded clause is a marginalize target's last mention.
    let mut built = 0usize;
    let mut pairs = if may_extend { batch_spine_pairs(&local, &touched) } else { 0 };
    let mut holds_completion = stop.completes(start);
    let mut i = start + 1;
    let mut batch_count = 1;
    while extends_past(i, batch_count) {
        if track_spine {
            mark_clause_levels(vtree, diagram_clauses.get(clauses[i]), on_spine, &mut touched);
        }
        local = eng.and_clause(local, diagram_clauses.get(clauses[i]))?;
        minimize_contract_only(eng, &mut local)?;
        holds_completion |= stop.completes(i);
        i += 1;
        batch_count += 1;
        if !eng.is_sat(&local)? { break; }
        // A disabled cap can never fire, and the spine union `batch_max_width`
        // reads is only tracked when a cap is on or the batch may extend.
        let width = if track_spine { batch_max_width(&local, &touched) } else { 0 };
        if may_extend {
            built += pairs;
            pairs = batch_spine_pairs(&local, &touched);
        }
        // The floor: below it the width cap stops every batch, extendable or
        // not, so only a batch that reached the floor within the cap extends.
        if batch_count < stop.floor {
            if width <= stop.width_cap { continue; }
            break;
        }
        // The extension.
        if !may_extend || holds_completion || width > stop.guard { break; }
        if batch_count.saturating_mul(pairs)
            >= built.saturating_add(BATCH_MERGE_COST_RATIO.saturating_mul(stop.acc_pairs()))
        {
            break;
        }
    }
    // Restore the scratch's all-false invariant.
    for &t in &touched { on_spine[t.idx()] = false; }
    Ok((Some(local), i))
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
    marginalize_targets: Option<&[VtreeIdx]>,
    acc_pairs: Option<usize>,
) -> Result<(Tdd, bool), OperationError> {
    let Some(a) = acc else {
        // First batch: the batch builder's output is already minimized. If targets
        // are set (single-batch step), marginalize inline so streamed levels
        // reach their final state, there being no conjunction to stream them
        // through.
        let mut result = batch;
        if let Some(targets) = marginalize_targets {
            eng.marginalize_levels(&mut result, targets)?;
        }
        return Ok((result, false));
    };

    let mut result = match marginalize_targets {
        Some(targets) => eng.and_marginalizing(a, batch, targets)?,
        None => eng.and(a, batch)?,
    };
    let (num, den) = BATCH_ABSORB_FACTOR;
    let absorbed = acc_pairs.is_some_and(|n| result.pair_count().saturating_mul(den) <= num.saturating_mul(n));

    state.adaptive_minimize(eng, &mut result)?;

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
    marginalize_targets: Option<&[VtreeIdx]>,
) -> Result<Tdd, OperationError> {
    let mut result = eng.and_clause(acc, clause)?;
    state.adaptive_minimize(eng, &mut result)?;
    if let Some(targets) = marginalize_targets {
        eng.marginalize_levels(&mut result, targets)?;
    }
    Ok(result)
}

#[cfg(test)]
#[path = "tests/batch.rs"]
mod tests;
