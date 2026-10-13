//! The compiler's minimize: every reduction it asks `Engine::reduce` for, and
//! the gate ([`MinimizeGate`]) that picks among them after each merge from the
//! result's size and its growth since the last prune, with the
//! contract-effectiveness throttle.
//!
//! Every reduction the compiler runs is built here, so a cofactor's compile
//! skips the content-twin scan in each of them.

use tididi::diagram::Tdd;
use tididi::limits::LimitConfig;
use tididi::reduce::{ContentTwinPolicy, ContentTwinSchedule, ReductionPlan};
use tididi::{Engine, OperationError};

use super::config::{MIN_SIZE_MULT, PRUNE_GROWTH_TRIGGER};

/// The full reduction, scanning content twins on `probe`'s schedule, or on
/// every call without one; a cofactor's compile skips the scan
/// (`skip_content_twins`, see [`CompileRun::cofactor`](super::CompileRun::cofactor)).
pub(crate) fn full_reduction(skip_content_twins: bool, probe: Option<&mut ContentTwinSchedule>) -> ReductionPlan<'_> {
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
fn minimize_prune_only(eng: &Engine, tdd: &mut Tdd) -> Result<(), OperationError> {
    eng.reduce(tdd, ReductionPlan::Prune)
}

/// Fallible inner-node twin contraction only.
pub(crate) fn minimize_contract_only(eng: &Engine, tdd: &mut Tdd) -> Result<(), OperationError> {
    eng.reduce(tdd, ReductionPlan::Contract)
}

// The contract throttle: when the median fraction of nodes the last few
// contractions removed is below a floor, the next few contractions the gate
// would run below the size threshold are skipped.

/// How many recent contractions the throttle's median is taken over; even,
/// so the median is the mean of the middle two.
const CONTRACT_HISTORY_SIZE: usize = 4;
const _: () = assert!(CONTRACT_HISTORY_SIZE.is_multiple_of(2));
/// The median fraction of nodes removed below which contraction is
/// throttled.
const CONTRACT_MIN_REMOVAL_FRACTION: f64 = 0.005;
/// How many contractions a throttle skips.
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
#[derive(Default)]
pub(crate) struct MinimizeGate {
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

    /// The fractions of nodes the last `CONTRACT_HISTORY_SIZE` contractions
    /// removed, a ring written at `contract_history_cursor`.
    contract_history: [f64; CONTRACT_HISTORY_SIZE],
    /// Whether every slot of `contract_history` holds a sample taken since
    /// the throttle was last rearmed.
    contract_history_full: bool,
    contract_history_cursor: usize,
    /// How many more contractions the throttle skips.
    contract_skip_remaining: usize,

    /// The content-twin scan schedule of the full reduction. It lives on the
    /// gate because each merge's result is a fresh diagram.
    content_twin_schedule: ContentTwinSchedule,
    /// Whether the full minimize skips the content-twin scan, as a recovery
    /// cofactor's compile does.
    pub(crate) skip_content_twins: bool,
}

impl MinimizeGate {
    /// The gate's decision; [`MinimizeAction`] says when each is taken.
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
        if self.contract_history_cursor == 0 {
            self.contract_history_full = true;
        }
    }

    /// Forget the recent removal fractions and let the next merge contract.
    /// Called after a full reduction, whose prune may have uncovered twins.
    fn rearm_contract_throttle(&mut self) {
        self.contract_skip_remaining = 0;
        self.contract_history_full = false;
        self.contract_history_cursor = 0;
    }

    /// The median of the recent removal fractions, or `None` until the ring
    /// is full: the throttle stays off until then.
    fn contract_removal_median(&self) -> Option<f64> {
        if !self.contract_history_full {
            return None;
        }
        let mut xs = self.contract_history;
        xs.sort_by(f64::total_cmp);
        let mid = CONTRACT_HISTORY_SIZE / 2;
        Some(0.5 * (xs[mid - 1] + xs[mid]))
    }

    /// Whether to skip this contraction, consuming one slot of the skip
    /// budget if so. Called only for
    /// [`MinimizeAction::ContractUnlessThrottled`].
    fn try_skip_contract(&mut self) -> bool {
        if self.contract_skip_remaining > 0 {
            self.contract_skip_remaining -= 1;
            return true;
        }
        if let Some(median) = self.contract_removal_median() && median < CONTRACT_MIN_REMOVAL_FRACTION {
            // Consume one slot immediately; the rest applies to future merges.
            self.contract_skip_remaining = CONTRACT_SKIP_COUNT - 1;
            return true;
        }
        false
    }

    /// Run the reduction the gate decides on for `result`, and re-anchor the
    /// gate on what it leaves.
    pub(crate) fn adaptive_minimize(
        &mut self, eng: &Engine, result: &mut Tdd,
    ) -> Result<(), OperationError> {
        let cur_nodes = result.node_count();
        let cur_retired = result.retired_marginal_slots();

        match self.decide(cur_nodes, cur_retired) {
            MinimizeAction::Full => {
                eng.reduce(result, full_reduction(self.skip_content_twins, Some(&mut self.content_twin_schedule)))?;
                self.record_full(
                    result.node_count(),
                    result.retired_marginal_slots(),
                );
                self.rearm_contract_throttle();
            }
            MinimizeAction::ContractOnly => {
                self.run_contract_and_record(eng, result, cur_nodes)?;
                self.record_threshold(
                    result.node_count(),
                    result.retired_marginal_slots(),
                );
            }
            MinimizeAction::PruneOnly => {
                minimize_prune_only(eng, result)?;
                self.record_prune(result.node_count(), result.retired_marginal_slots());
                // The throttle is not rearmed: a prune alone removes dead
                // nodes, not twins, so the removal fractions still hold.
            }
            MinimizeAction::ContractUnlessThrottled => {
                if !self.try_skip_contract() {
                    self.run_contract_and_record(eng, result, cur_nodes)?;
                }
            }
        }
        Ok(())
    }

    /// Run twin contraction alone and record the fraction of nodes it
    /// removed.
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
