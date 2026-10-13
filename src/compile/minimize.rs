//! The compiler's minimize: every reduction it asks `Engine::reduce` for, and
//! the gate ([`CompileState`]) that picks among them after each merge from the
//! result's size and its growth since the last prune, with the
//! contract-effectiveness throttle.

// ── Solver-side minimize entry ───────────────────────────────────────────────
//
// Every minimize the compiler runs goes through here, so the one piece of
// policy the library cannot know — that a Shannon-split recovery cofactor's
// compile skips the content-twin pass — is filled in exactly once.
// A direct `Engine::reduce` call without the compiler plan would
// fork that policy.

use tididi::reduce::{ContentTwinSchedule, ContentTwinPolicy, ReductionPlan};
use tididi::Engine;
use tididi::limits::LimitConfig;
use tididi::OperationError;
use tididi::diagram::Tdd;

use super::config::{MIN_SIZE_MULT, PRUNE_GROWTH_TRIGGER};

/// Full reduction with the recovery policy and optional persistent scan schedule.
pub(crate) fn full_reduction(skip_content_twins: bool, probe: Option<&mut ContentTwinSchedule>) -> ReductionPlan<'_> {
    // Recovery recompiles cofactors under memory pressure; scanning a diagram
    // that is about to be split or dropped adds no useful work.
    let twins = if skip_content_twins {
        ContentTwinPolicy::Skip
    } else {
        probe.map_or(ContentTwinPolicy::Fresh, ContentTwinPolicy::Adaptive)
    };
    ReductionPlan::Full(twins)
}

/// Full canonical reduction, with the stop rules off; `skip_content_twins`
/// as [`full_reduction`] takes it.
///
/// An allocation refused here is the end of this compile: the diagram is whole
/// but cannot be reduced, so the caller gives up and the binary reports it as
/// an out-of-memory run.
pub(crate) fn minimize_full(eng: &Engine, tdd: &mut Tdd, skip_content_twins: bool) -> Result<(), OperationError> {
    // This entry has nowhere to report a cut, so a reduce-walk poll firing
    // inside it would turn a budget expiry into a give-up the caller cannot
    // tell from a real allocation refusal. The whole axis goes, conditional
    // bound and schedule included — clearing the wall alone leaves a rope armed
    // by an enclosing step in force here.
    let _shield = eng.limits().edit(LimitConfig::without_stop_rules);
    eng.reduce(tdd, full_reduction(skip_content_twins, None))
}

/// Fallible prune (+ marginal-slot compaction) without contraction.
pub(crate) fn minimize_prune_only(eng: &Engine, tdd: &mut Tdd) -> Result<(), OperationError> {
    eng.reduce(tdd, ReductionPlan::Prune)
}

/// Fallible inner-node twin contraction only.
pub(crate) fn minimize_contract_only(eng: &Engine, tdd: &mut Tdd) -> Result<(), OperationError> {
    eng.reduce(tdd, ReductionPlan::Contract)
}

// ── Adaptive minimize constants ──────────────────────────────────────────────

// Contract-effectiveness throttle: twin contraction is a large share of
// compile time yet usually removes a few percent of nodes. When the median
// removal fraction of the last few contractions is tiny, the next few
// contract-only merges skip it. Full minimize (with prune) always runs.
/// Size of the ring buffer used to smooth removal-rate noise.
const CONTRACT_HISTORY_SIZE: usize = 4;
/// Median removal-fraction threshold. Below this, contract is throttled.
/// 0.005 = 0.5% of nodes removed per call.
const CONTRACT_THRESHOLD_PCT: f64 = 0.005;
/// Number of contract-only merges to skip after the throttle triggers.
const CONTRACT_SKIP_COUNT: usize = 4;

/// What the gate decided this call. Each variant maps onto a minimize variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MinimizeAction {
    /// Over the size threshold and grown since the last prune: the full
    /// reduction (prune, inner and leaf twin contraction, content twins).
    Full,
    /// Under the threshold, grown since the last prune: prune only.
    PruneOnly,
    /// Over the threshold, not grown since the last prune: inner-node twin
    /// contraction only.
    ContractOnly,
    /// Under the threshold and not grown: inner-node twin contraction, unless
    /// the throttle finds recent contractions removed too little.
    ContractUnlessThrottled,
}

/// The adaptive-minimize gate a bottom-up pass carries from merge to merge.
///
/// Decides when to run full minimize
/// (prune + twin contraction) vs. lightweight variants. Uses
/// `size_threshold` and `last_prune_nodes` to skip expensive passes when
/// the TDD isn't growing much between merges. A fresh state (all zero) starts
/// with `size_threshold == 0`, so the size trigger fires on the first
/// minimize; it then self-anchors from the post-minimize size.
#[derive(Default)]
pub(crate) struct CompileState {
    /// When `node_count` exceeds this, the expensive tier is eligible. Reset
    /// after each full minimize to `post_size * MIN_SIZE_MULT`.
    size_threshold: usize,
    /// `Tdd::retired_marginal_slots()` snapshot paired with `size_threshold` so slot
    /// collection since the snapshot doesn't deflate the gate metric.
    size_threshold_retired: usize,
    /// Node count after the last prune. Prune re-fires when current nodes
    /// exceed `last_prune_nodes * PRUNE_GROWTH_TRIGGER`.
    last_prune_nodes: usize,
    /// `Tdd::retired_marginal_slots()` snapshot paired with `last_prune_nodes`.
    last_prune_retired: usize,

    /// Contract-effectiveness throttle state.
    /// Ring buffer of the last `CONTRACT_HISTORY_SIZE` contract removal
    /// fractions (removed / pre_nodes). When the median drops below
    /// `CONTRACT_THRESHOLD_PCT`, we skip `minimize_contract_only` on the next
    /// `CONTRACT_SKIP_COUNT` merges (since contract is spending instructions
    /// for near-zero gain). A prune event resets the skip counter so the
    /// next merge always runs contract: fresh prunes can expose new twins.
    contract_history: [f64; CONTRACT_HISTORY_SIZE],
    contract_history_len: usize,
    contract_history_cursor: usize,
    contract_skip_remaining: usize,

    /// Content-twin scan schedule, handed to the full-minimize branch in
    /// `adaptive_minimize`. Lives on the state because `result` is a fresh `Tdd`
    /// every clause, so per-`Tdd` state would silently reset to "always scan"
    /// (= eager) each merge.
    c2_probe: ContentTwinSchedule,
    /// Whether the full minimize skips the content-twin scan, as a recovery
    /// cofactor's compile does.
    pub(crate) skip_content_twins: bool,
}

impl CompileState {
    /// The gate decision, pure: prune when the diagram has grown since the last
    /// prune, contract when it is over the size threshold.
    ///
    /// `pre_nodes` is `node_count()` before this minimize; `cur_retired` is
    /// `Tdd::retired_marginal_slots()` at the same instant (the slot-collection offset).
    fn decide(&self, pre_nodes: usize, cur_retired: usize) -> MinimizeAction {
        let prune_collected = cur_retired.saturating_sub(self.last_prune_retired);
        let do_prune = self.last_prune_nodes == 0
            || (pre_nodes + prune_collected) as f64
                > self.last_prune_nodes as f64 * PRUNE_GROWTH_TRIGGER;

        let threshold_collected = cur_retired.saturating_sub(self.size_threshold_retired);
        let threshold_exceeded = pre_nodes + threshold_collected > self.size_threshold;

        match (threshold_exceeded, do_prune) {
            (true, true) => MinimizeAction::Full,
            (true, false) => MinimizeAction::ContractOnly,
            (false, true) => MinimizeAction::PruneOnly,
            (false, false) => MinimizeAction::ContractUnlessThrottled,
        }
    }

    /// Re-anchor both baselines after a full minimize (prune ran and the
    /// threshold is reset to `post_nodes * MIN_SIZE_MULT`).
    fn record_full(&mut self, post_nodes: usize, post_retired: usize) {
        self.record_prune(post_nodes, post_retired);
        self.record_threshold(post_nodes, post_retired);
    }

    /// Re-anchor only the size threshold (used on the above-threshold
    /// contract-only branch, where prune did not run so its baseline is intact).
    fn record_threshold(&mut self, post_nodes: usize, post_retired: usize) {
        self.size_threshold = (post_nodes as f64 * MIN_SIZE_MULT) as usize;
        self.size_threshold_retired = post_retired;
    }

    /// Re-anchor only the prune baseline after a prune-only pass.
    fn record_prune(&mut self, post_nodes: usize, post_retired: usize) {
        self.last_prune_nodes = post_nodes;
        self.last_prune_retired = post_retired;
    }

    /// Append a removal fraction to the ring buffer.
    fn record_contract_removal(&mut self, fraction: f64) {
        self.contract_history[self.contract_history_cursor] = fraction;
        self.contract_history_cursor =
            (self.contract_history_cursor + 1) % CONTRACT_HISTORY_SIZE;
        if self.contract_history_len < CONTRACT_HISTORY_SIZE {
            self.contract_history_len += 1;
        }
    }

    /// Forget recent contract-effectiveness samples and let the next merge
    /// run a contract pass. Called right after a prune, since prune may have
    /// uncovered fresh twins that contract would now find.
    fn rearm_contract_throttle(&mut self) {
        self.contract_skip_remaining = 0;
        self.contract_history_len = 0;
        self.contract_history_cursor = 0;
    }

    /// Median of the ring buffer (only over populated slots).
    /// Returns `None` before the buffer is full — the throttle is off until
    /// we have enough samples to be confident contract is unproductive.
    fn contract_removal_median(&self) -> Option<f64> {
        if self.contract_history_len < CONTRACT_HISTORY_SIZE {
            return None;
        }
        let mut xs = self.contract_history;
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = xs.len();
        Some(if n % 2 == 1 { xs[n / 2] } else { 0.5 * (xs[n / 2 - 1] + xs[n / 2]) })
    }

    /// Decide whether to skip this contract-only call, consuming one slot
    /// of the skip budget if we do. Called only on the contract-only branch
    /// of `adaptive_minimize` (sub-threshold, no prune).
    fn try_skip_contract(&mut self) -> bool {
        if self.contract_skip_remaining > 0 {
            self.contract_skip_remaining -= 1;
            return true;
        }
        if let Some(median) = self.contract_removal_median() && median < CONTRACT_THRESHOLD_PCT {
            // Consume one slot immediately; the rest applies to future merges.
            self.contract_skip_remaining = CONTRACT_SKIP_COUNT - 1;
            return true;
        }
        false
    }

    /// Choose and apply the appropriate minimize variant based on TDD growth.
    ///
    /// Balances canonicality (expensive) vs. speed (cheap):
    ///
    /// ```text
    ///                     ┌── prune needed → full minimize (prune + contract)
    ///   Above threshold ──┤
    ///                     └── recently pruned → contract only
    ///                     [then reset threshold to post_size × multiplier]
    ///
    ///                     ┌── prune needed → prune only (skip contract)
    ///   Below threshold ──┤
    ///                     └── recently pruned → contract, unless throttled
    /// ```
    ///
    /// Key insight: twin contraction is the most expensive phase. Below
    /// the threshold, prune alone keeps the TDD clean enough for
    /// subsequent merges without paying the contraction cost.
    pub(crate) fn adaptive_minimize(
        &mut self, eng: &Engine, result: &mut Tdd,
    ) -> Result<(), OperationError> {
        // Snapshot of the current retired total. Used for the threshold-offset
        // gating in `decide`: each comparison adds the slots retired since the
        // stored baseline's snapshot to `node_count()` so that slot-pruning
        // does not deflate the gate metric and delay triggers.
        let cur_nodes = result.node_count();
        let cur_retired = result.retired_marginal_slots();

        // The prune-growth + size-threshold formulas live in `decide`; the
        // contract throttle layers on the arm bodies below.
        match self.decide(cur_nodes, cur_retired) {
            MinimizeAction::Full => {
                eng.reduce(result, full_reduction(self.skip_content_twins, Some(&mut self.c2_probe)))?;
                self.record_full(
                    result.node_count(),
                    result.retired_marginal_slots(),
                );
                self.rearm_contract_throttle();
            }
            MinimizeAction::ContractOnly => {
                // Above threshold + no prune: contract is how we keep canonical
                // form, so we always run it (and record its effectiveness), then
                // re-anchor the threshold (prune baseline is untouched).
                self.run_contract_and_record(eng, result, cur_nodes)?;
                self.record_threshold(
                    result.node_count(),
                    result.retired_marginal_slots(),
                );
            }
            MinimizeAction::PruneOnly => {
                minimize_prune_only(eng, result)?;
                self.record_prune(result.node_count(), result.retired_marginal_slots());
                // Don't rearm contract throttle here: prune removes dead nodes only,
                // not twins, so the effectiveness history is still valid.
                // Rearming on every sub-threshold prune would cause contract to run
                // unconditionally after each prune event (throttle needs `CONTRACT_HISTORY_SIZE`
                // samples before it can suppress), defeating throttle at small triggers.
            }
            MinimizeAction::ContractUnlessThrottled => {
                if !self.try_skip_contract() {
                    self.run_contract_and_record(eng, result, cur_nodes)?;
                }
                // else: skipped contract entirely — cheap no-op this merge.
            }
        }
        Ok(())
    }

    /// Run contract-only minimize (twin contraction) and push the removal fraction
    /// onto the history ring. Used on every merge where we decide to contract.
    fn run_contract_and_record(
        &mut self, eng: &Engine, result: &mut Tdd,
        pre_nodes: usize,
    ) -> Result<(), OperationError> {
        minimize_contract_only(eng, result)?;
        let post_nodes = result.node_count();
        let removed = pre_nodes.saturating_sub(post_nodes);
        let fraction = if pre_nodes == 0 { 0.0 } else { removed as f64 / pre_nodes as f64 };
        self.record_contract_removal(fraction);
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/minimize.rs"]
mod tests;
