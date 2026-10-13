//! Folding one vtree node's clauses into its accumulator, a batch at a time.
//!
//! A step's clauses are not conjoined one by one into the accumulator: they are
//! built into a batch TDD first, and the batch is merged. `build_one_batch`
//! decides where a batch stops (clause count, width cap, UNSAT),
//! `merge_batch` merges one batch and says whether the accumulator went UNSAT,
//! and `batch_accumulate` is the fold over the two. The width cap, the spine
//! marking that reads a batch's width, and the extra-TDD injection at a node
//! are here as well. `step.rs` decides which node is folded and when.

use std::sync::Arc;

use tididi::Engine;
use tididi::{Literal, OperationError};
use tididi::diagram::Tdd;
use tididi::vtree::{Vtree, VtreeIdx};
use super::minimize::{CompileState, minimize_contract_only};
use super::config::{BATCH_SIZE, BATCH_WIDTH_LIMIT, UNCAPPED_ACC_WIDTH};
use super::marginalize::summable;
use super::schedule::intra_batch_completions;
use super::step::{DiagramClauses, PassState, StepContext};
use crate::budget::deadline_passed;

// ── `batch_accumulate` ────────────────────────────────────────────────────────

/// Conjoin clause TDDs into an accumulator using batched apply.
///
/// Two-level minimize strategy:
/// - **Within** a batch: clauses are conjoined one-by-one via the specialized
///   `and_clause` (TDD × clause conjunction), followed by
///   `minimize_contract_only`.
///   Prune is unnecessary because `and_clause` output has no unreachable
///   nodes (the `need_dt` filter ensures dead complement nodes are never created).
/// - **Between** batches: the batch result is merged into the accumulator and
///   [`CompileState::adaptive_minimize`] picks the minimize to run.
///
/// Checks `ctx.job.deadline` between batches and returns
/// `OperationError::Stopped` once it has passed.
///
/// Early-exits on UNSAT (empty conjunction detected).
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

    // Per-clause last-mention scheduler: precompute, for each clause position
    // i in this X-batch, which sub-vtrees (S ∈ marginalize_at[X]) have their
    // last-mention at clauses[i]. Streaming for S then fires at the *sub-batch
    // boundary covering clauses[i]*, rather than waiting for the final merge.
    let mut streaming = marginalize_targets.map(|targets| {
        let clause_lits: Vec<&[Literal]> =
            clauses.iter().map(|&ci| ctx.clauses.get(ci)).collect();
        StreamTargets {
            completions: intra_batch_completions(&clause_lits, vtree, targets),
            set: Vec::new(),
        }
    });

    // A live accumulator makes a size-1 batch's TDD dead on arrival — the merge
    // conjoins the clause straight into `acc` via the clause-specialized apply —
    // so the builder skips that build and returns `None`.
    let mut i = 0;
    while i < clauses.len() {
        if deadline_passed(ctx.job.deadline) { return Err(OperationError::Stopped); }
        let start_clause = i;
        let mut local;
        (local, i) = build_one_batch(eng, ctx.clauses, vtree, clauses, start_clause, acc.as_ref(), &mut state.on_spine)?;
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
        if merge_batch(eng, local, first_clause, &mut acc, &mut state.minimize, merge_targets.as_deref())? {
            return Ok(acc);
        }
    }
    Ok(acc)
}

/// The marginalize targets that become final within each sub-batch of one
/// node's fold.
struct StreamTargets {
    /// Per clause position, the sub-vtrees whose last mention it is, children
    /// before parents.
    completions: Vec<Vec<VtreeIdx>>,
    /// What `completed_in` last returned.
    set: Vec<VtreeIdx>,
}

impl StreamTargets {
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

// ── `merge_batch` ─────────────────────────────────────────────────────────────

/// Merge one built batch into `acc`. Returns `Ok(true)` when the merged
/// accumulator is UNSAT, so the fold can stop complete, and `Ok(false)` to
/// continue.
///
/// `local` is `None` when the builder skipped a size-1 batch's TDD, which it
/// does only for a live accumulator; `first_clause` is then conjoined directly.
fn merge_batch(
    eng: &Engine,
    local: Option<Tdd>,
    first_clause: &[Literal],
    acc: &mut Option<Tdd>,
    state: &mut CompileState,
    merge_targets: Option<&[VtreeIdx]>,
) -> Result<bool, OperationError> {
    // Merge batch result into the accumulator. Propagates `OperationError::OverBudget` if
    // `apply_and` cannot reserve scratch; the pass aborts and the compile gives up.
    //
    // A size-1 batch merged into a live accumulator is a single clause — conjoin it
    // directly via the clause-specialized apply (touches only the clause's nodes; the
    // grid merge would walk both operands' full level grids). This is the sole
    // single-clause path: it is correctness-identical to the grid route and no
    // slower in aggregate. That case needs no batch TDD at all, so the builder
    // hands back `None` and never builds one; every other case
    // (a multi-clause batch, or the first batch that becomes the accumulator) goes
    // through the grid merge.
    let acc_in = acc.take();
    *acc = Some(match local {
        Some(batch) => merge_batch_into_acc(eng, acc_in, batch, state, merge_targets)?,
        None => merge_clause_into_acc(
            eng,
            acc_in.expect("a skipped size-1 batch build implies a live accumulator"),
            first_clause,
            state,
            merge_targets,
        )?,
    });
    // UNSAT accumulator: the step's conjunction is zero regardless of the remaining
    // clauses, so this is a sound early-Complete.
    Ok(!eng.is_sat(acc.as_ref().unwrap())?)
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

// ── `batch_max_width` ─────────────────────────────────────────────────────────

/// `local.max_width()` for a batch diagram under construction, read off the
/// union of the folded clauses' spines (`spine`) instead of sweeping every
/// vtree level. Returns exactly the number `Tdd::max_width` would — this is a cost
/// change, not a behavior change, so the flush decisions in `build_one_batch`
/// are bit-identical to the sweeping version.
///
/// Why it is exact. Three facts about a batch diagram, all local to
/// `build_one_batch`:
///
/// 1. **Off-spine levels start trivial.** The batch starts as
///    `Tdd::clause(first clause)`, which gives every internal level outside
///    that clause's spine a single One node (width 1) and every leaf level no
///    stored nodes at all (width 0).
/// 2. **Only spine levels ever grow.** `conjoin_clause_owned` rebuilds
///    exactly the clause's spine levels and carries every other level through
///    verbatim; `minimize_contract_only` only merges twins, which can shrink a
///    level but never widen one. So a level outside the running spine union
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

// ── `build_one_batch` ─────────────────────────────────────────────────────────

/// Build a single batch TDD by conjoining clauses one-by-one starting at `clauses[start]`,
/// stopping when the batch reaches `BATCH_SIZE` clauses, becomes UNSAT, or its width
/// exceeds [`batch_width_cap`] of the accumulator `acc`'s width (so the later
/// merge with the accumulator stays cheap). Returns the batch TDD and the index
/// of the next unconsumed clause (`>= start + 1`, since the first clause is always
/// taken). Each conjoin minimizes (no prune) to keep the running width honest before
/// the width check. `on_spine` is the pass's spine-marking scratch.
///
/// A live accumulator has no use for a size-1 batch's TDD:
/// `merge_clause_into_acc` conjoins the clause through the clause-specialized
/// apply and never looks at a prebuilt clause TDD. Whether the batch stops at
/// one clause is decided by the two gates in `extends_past` — clause-list end
/// and `BATCH_SIZE` — both of which are known before any TDD exists, so when
/// `acc` is live and those gates say "size 1" the `Tdd::clause` build is
/// skipped outright and the returned TDD is `None` (with `next == start + 1`).
fn build_one_batch(
    eng: &Engine,
    diagram_clauses: &DiagramClauses,
    vtree: &Arc<Vtree>,
    clauses: &[usize],
    start: usize,
    acc: Option<&Tdd>,
    on_spine: &mut Vec<bool>,
) -> Result<(Option<Tdd>, usize), OperationError> {
    let batch_width_cap = batch_width_cap(acc.map_or(0, Tdd::max_width));
    // Does the batch reach `clauses[i]`, given it already holds `batch_count`
    // clauses? The single statement of the batch's *content-independent* stop
    // conditions — evaluated once up front for the skip decision and once per
    // round by the fold below, never restated.
    let extends_past =
        |i: usize, batch_count: usize| i < clauses.len() && batch_count < BATCH_SIZE;

    // Size-1 batch the caller will discard: nothing below would read `local`, so
    // don't build it. (The content-dependent stops — UNSAT and the width cap —
    // are checked *after* a second clause is folded in, so they can never turn
    // a batch the gates above admit into a size-1 batch.)
    if acc.is_some() && !extends_past(start + 1, 1) {
        return Ok((None, start + 1));
    }

    let mut local = eng.clause(vtree, diagram_clauses.get(clauses[start]).iter().copied())?;
    // Spine bookkeeping: the running union of the folded clauses' spines, from
    // which the width check reads the batch's max width instead of sweeping
    // every level (`batch_max_width`). Tracked only when the width cap is on.
    let track_spine = batch_width_cap != usize::MAX;
    let mut touched: Vec<VtreeIdx> = Vec::new();
    // All false on entry: only spine entries are ever set, and they are reset
    // from `touched` before returning. An error return skips the reset, but
    // an error ends the pass, whose state the scratch is part of.
    if track_spine {
        if on_spine.len() < vtree.num_nodes() { on_spine.resize(vtree.num_nodes(), false); }
        mark_clause_levels(vtree, diagram_clauses.get(clauses[start]), on_spine, &mut touched);
    }
    let mut i = start + 1;
    let mut batch_count = 1;
    while extends_past(i, batch_count) {
        if track_spine {
            mark_clause_levels(vtree, diagram_clauses.get(clauses[i]), on_spine, &mut touched);
        }
        local = eng.and_clause(local, diagram_clauses.get(clauses[i]))?;
        minimize_contract_only(eng, &mut local)?;
        i += 1;
        batch_count += 1;
        if !eng.is_sat(&local)? { break; }
        // A disabled cap can never fire, and the spine union `batch_max_width`
        // reads is only tracked when the cap is on.
        if track_spine && batch_max_width(&local, &touched) > batch_width_cap { break; }
    }
    // Restore the scratch's all-false invariant.
    for &t in &touched { on_spine[t.idx()] = false; }
    Ok((Some(local), i))
}

// ── `batch_width_cap` ─────────────────────────────────────────────────────────

/// Maximum allowed batch-TDD width before the batch should be flushed into the
/// accumulator: no cap (`usize::MAX`) while the accumulator is at most
/// [`UNCAPPED_ACC_WIDTH`] wide, [`BATCH_WIDTH_LIMIT`] above that.
///
/// Rationale: wide batch × wide accumulator triggers the most expensive apply_and
/// path (NxM cross-product). When `acc` is already wide, the identity fast-paths
/// inside `apply_and` matter more, so we keep batches narrow; floored at
/// `BATCH_WIDTH_LIMIT` so batches never degenerate to single clauses, which
/// would defeat batching entirely.
fn batch_width_cap(acc_width: usize) -> usize {
    if acc_width <= UNCAPPED_ACC_WIDTH { usize::MAX } else { BATCH_WIDTH_LIMIT }
}

// ── `merge_batch_into_acc` ────────────────────────────────────────────────────

/// Merge a batch TDD into the accumulator, marginalizing `marginalize_targets`
/// inside the conjunction, then let [`CompileState::adaptive_minimize`] pick
/// the minimize to run.
///
/// Returns `Err(OperationError::OverBudget)` if the underlying `apply_and`
/// could not reserve memory (OS allocator refused); the bottom-up pass
/// aborts on it.
fn merge_batch_into_acc(
    eng: &Engine,
    acc: Option<Tdd>,
    batch: Tdd,
    state: &mut CompileState,
    marginalize_targets: Option<&[VtreeIdx]>,
) -> Result<Tdd, OperationError> {
    let Some(a) = acc else {
        // First batch: the batch builder's output is already minimized. If targets
        // are set (single-batch step), marginalize inline so streamed levels
        // reach their final state, there being no conjunction to stream them
        // through.
        let mut result = batch;
        if let Some(targets) = marginalize_targets {
            eng.marginalize_levels(&mut result, targets)?;
        }
        return Ok(result);
    };

    // The library's conjunction already skips the levels the batch cannot
    // change, so every batch goes through the generic merge.
    let mut result = match marginalize_targets {
        Some(targets) => eng.and_marginalizing(a, batch, targets)?,
        None => eng.and(a, batch)?,
    };

    state.adaptive_minimize(eng, &mut result)?;

    Ok(result)
}

// ── `merge_clause_into_acc` ───────────────────────────────────────────────────

/// Specialized merge for a size-1 batch: conjoin `clause` directly into `acc`
/// via the clause-specialized apply instead of the generic grid merge, minimize,
/// then marginalize `marginalize_targets` (the grid merge marginalizes inside
/// its conjunction, before minimizing).
///
/// Takes the accumulator by value, not as an `Option`: this path exists because
/// there IS an accumulator to fold the clause into. A size-1 batch with no
/// accumulator has nothing to conjoin — the clause TDD simply becomes the
/// accumulator — which is `merge_batch_into_acc`'s empty-`acc` case verbatim, so
/// the caller routes it there and no clause TDD is built here or passed in.
fn merge_clause_into_acc(
    eng: &Engine,
    acc: Tdd,
    clause: &[Literal],
    state: &mut CompileState,
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
