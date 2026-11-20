//! The compiler's minimize: every reduction it asks `Engine::reduce` for, and
//! the gate ([`MinimizeGate`]) that picks among them after each merge from the
//! result's size and its growth since the last prune, with the
//! contract-effectiveness throttle.
//!
//! Every reduction the compiler runs is built here, so a cofactor's compile
//! skips the content-twin scan in each of them.

use tididi::{Engine, OperationError};
use tididi::diagram::Tdd;
use tididi::limits::LimitConfig;
use tididi::reduce::{ContentTwinPolicy, ContentTwinSchedule, ReductionPlan};

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

/// The full reduction, with the stop rules off; `skip_content_twins` as
/// [`full_reduction`] takes it. Canonical only without a marginal level: with
/// one, the content-twin scan makes the result depend on where scans fell
/// between the conjunctions that built the diagram.
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

/// After a full minimize, the size threshold is this multiple of the
/// diagram's node count.
const MIN_SIZE_MULT: f64 = 1.1;

/// A prune is due once the diagram has grown past this multiple of its node
/// count after the last prune.
const PRUNE_GROWTH_TRIGGER: f64 = 1.05;

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
/// The work units per node of a contraction that walked its whole diagram
/// rather than the levels a merge touched. Such walks measured 1.9 to 2.0
/// units per node; incremental contractions stayed under 1.0, and ones that
/// removed a third or more of a diagram reached 1.64.
const CONTRACT_WALK_WORK_PER_NODE: f64 = 1.5;

/// The work units per pair of its result from which a fold's last merge scans
/// content twins whatever the gate and the schedule say. On model counting
/// instances, the folds a scan paid off on had spent 10 to 570 units per pair
/// and those it did not at most 9.3; 16 keeps a margin above the latter.
const SCAN_FOLD_WORK_PER_PAIR: u64 = 16;

/// The pairs [`MinimizeGate::fold_paid_for_scan`] adds to a result's own for
/// what a full reduction costs whatever the diagram's size. The reduction
/// walks every level: on model counting instances one of a diagram of 1 to 3
/// nodes cost 240 to 300 instructions per level, and a fit of the reductions'
/// instructions to their pairs gave about 330,000 plus 370 per pair, about
/// what 900 pairs cost.
const SCAN_FIXED_COST_PAIRS: u64 = 900;

/// The fraction of nodes below which a scan [`SCAN_FOLD_WORK_PER_PAIR`] forced
/// counts as wasted and doubles the work per pair the next one needs. On small
/// count diagrams the test holds at nearly every fold end while the scans
/// remove under 0.1% of the nodes.
const SCAN_MIN_REMOVAL_FRACTION: f64 = 0.01;

/// The most nodes a merge inside a fold that summed levels out may hold and
/// still scan content twins. Summing out leaves count nodes with equal
/// contents, and on small diagrams the scan halves the accumulator the fold's
/// next conjunctions pay for; on larger ones a scan there can leave the fold's
/// end with more pairs than the fold-end scan alone, which every later fold
/// pays for.
const MID_FOLD_SCAN_MAX_NODES: usize = 4096;

/// The most nodes the reduced result of a summed-out merge inside a fold may
/// hold for the merge to scan a copy of it, when the merge held more than
/// [`MID_FOLD_SCAN_MAX_NODES`] and the fold's work pays for a scan
/// ([`MinimizeGate::fold_paid_for_scan`]). The copy is kept only when the
/// scan left at most a quarter of the nodes. Kept scans on model counting
/// instances had removed 84 to 98% of the nodes; scans that removed 8 to 46%
/// left the fold's end larger. The bound caps what a discarded copy costs.
const MID_FOLD_TRIAL_MAX_NODES: usize = 65_536;

/// The most doublings [`SCAN_MIN_REMOVAL_FRACTION`] applies.
const SCAN_MAX_BACKOFF: u32 = 16;

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

/// Skips contractions while the recent ones on a diagram removed too little.
#[derive(Clone, Default)]
pub(crate) struct ContractThrottle {
    /// The fractions of nodes the last `CONTRACT_HISTORY_SIZE` contractions
    /// removed, a ring written at `cursor`.
    history: [f64; CONTRACT_HISTORY_SIZE],
    /// Whether every slot of `history` holds a sample taken since the
    /// throttle was last rearmed.
    full: bool,
    cursor: usize,
    /// How many more contractions the throttle skips.
    skip_remaining: usize,
    /// Whether `history` has been full once on this diagram. Until then the
    /// gate's shared throttle decides its contractions.
    warm: bool,
    /// The pair count of the diagram when the step that made it ended.
    pub(crate) pairs: usize,
    /// Whether the next contraction runs whatever `history` says
    /// ([`Self::reopen`]).
    trial: bool,
}

impl ContractThrottle {
    /// Append a removal fraction to the ring buffer.
    fn record(&mut self, fraction: f64) {
        if self.trial {
            self.trial = false;
            if fraction >= CONTRACT_MIN_REMOVAL_FRACTION {
                self.full = false;
                self.cursor = 0;
            }
        }
        self.history[self.cursor] = fraction;
        self.cursor = (self.cursor + 1) % CONTRACT_HISTORY_SIZE;
        if self.cursor == 0 {
            self.full = true;
            self.warm = true;
        }
    }

    /// Forget the recent removal fractions and let the next merges contract
    /// until the ring refills. Called after a full reduction, whose prune may
    /// have uncovered twins.
    fn rearm(&mut self) {
        self.skip_remaining = 0;
        self.full = false;
        self.cursor = 0;
    }

    /// Let the next contraction run after a full reduction, whose prune may
    /// have uncovered twins. A full ring keeps its fractions unless that
    /// contraction removes at least [`CONTRACT_MIN_REMOVAL_FRACTION`]: a
    /// diagram's ring refills only from its own contractions, and on a large
    /// diagram each of those can cost as much as the full reduction while
    /// removing nothing.
    fn reopen(&mut self) {
        if self.full {
            self.skip_remaining = 0;
            self.trial = true;
        } else {
            self.rearm();
        }
    }

    /// The median of the recent removal fractions, or `None` until the ring
    /// is full: the throttle stays off until then.
    fn median(&self) -> Option<f64> {
        if !self.full {
            return None;
        }
        let mut xs = self.history;
        xs.sort_by(f64::total_cmp);
        let mid = CONTRACT_HISTORY_SIZE / 2;
        Some(0.5 * (xs[mid - 1] + xs[mid]))
    }

    /// Whether to skip this contraction, consuming one slot of the skip
    /// budget if so. Called only for
    /// [`MinimizeAction::ContractUnlessThrottled`].
    fn try_skip(&mut self) -> bool {
        if self.trial {
            return false;
        }
        if self.skip_remaining > 0 {
            self.skip_remaining -= 1;
            return true;
        }
        if let Some(median) = self.median() && median < CONTRACT_MIN_REMOVAL_FRACTION {
            // Consume one slot immediately; the rest applies to future merges.
            self.skip_remaining = CONTRACT_SKIP_COUNT - 1;
            return true;
        }
        false
    }
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

    /// The contraction throttle of the diagram being reduced. A pass keeps
    /// one per vtree node's diagram and hands it on with the diagram
    /// ([`Self::swap_throttle`]), so removals measured on one diagram do not
    /// throttle or rearm another's contractions.
    throttle: ContractThrottle,
    /// The throttle over every diagram's contractions, which decides for a
    /// diagram whose own throttle is not yet warm: a new diagram is small,
    /// and on small diagrams a contraction's cost is the per-level walk,
    /// which the removals on other small diagrams predict.
    shared: ContractThrottle,
    /// How many more contractions on diagrams of `walk_skip_min_nodes` to
    /// `walk_skip_max_nodes` nodes the gate skips, on any diagram, after a
    /// contraction walked a diagram of a quarter of the upper bound and
    /// removed under [`CONTRACT_MIN_REMOVAL_FRACTION`]. A full reduction does
    /// not clear it: the walks that remove nothing come on fresh diagrams,
    /// between full reductions of other ones. A diagram more than four times
    /// the walked one's size is not skipped: it grew through merges the walk
    /// never saw, and its contraction can remove nearly half of it.
    walk_skip_remaining: usize,
    walk_skip_min_nodes: usize,
    walk_skip_max_nodes: usize,

    /// The content-twin scan schedule of the full reduction. It lives on the
    /// gate because each merge's result is a fresh diagram.
    content_twin_schedule: ContentTwinSchedule,
    /// Whether the full minimize skips the content-twin scan, as a recovery
    /// cofactor's compile and a pass that sums nothing out do.
    pub(crate) skip_content_twins: bool,
    /// The engine's work clock when the current fold started
    /// ([`Self::start_fold`]).
    fold_start_work: u64,
    /// How many times [`SCAN_FOLD_WORK_PER_PAIR`] is doubled: once for each
    /// forced scan in a row that removed too little.
    scan_backoff: u32,
    /// Whether the current fold is the root's, set by the step that runs it.
    /// Nothing folds the root's diagram again, so where its scans fall moves
    /// only its own work and size: any of its merges may scan when the work
    /// since the fold's start or its last scan pays for it.
    pub(crate) fold_at_root: bool,
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

    /// Make `throttle` the one the gate's contractions read and record, and
    /// return the one it held.
    pub(crate) fn swap_throttle(&mut self, throttle: ContractThrottle) -> ContractThrottle {
        std::mem::replace(&mut self.throttle, throttle)
    }

    /// Run the reduction the gate decides on for `result`, and re-anchor the
    /// gate on what it leaves.
    ///
    /// Only the last merge of a fold (`fold_end`) may scan content twins,
    /// except in the root's fold ([`Self::fold_at_root`]), where any merge may
    /// when [`Self::fold_paid_for_scan`] and then [`Self::root_scan_pays`]
    /// hold, and at a merge that summed levels out (`summed_out`) into at most
    /// [`MID_FOLD_SCAN_MAX_NODES`] nodes, or, outside the root's fold, into a
    /// reduced result of at most [`MID_FOLD_TRIAL_MAX_NODES`] when the scan
    /// leaves at most a quarter of them. Contraction commutes with conjunction
    /// and summing out, a content-twin merge does not, so a scan inside a fold
    /// makes the fold's result depend on its batch boundaries.
    pub(crate) fn adaptive_minimize(
        &mut self, eng: &Engine, result: &mut Tdd, fold_end: bool, summed_out: bool,
    ) -> Result<(), OperationError> {
        let cur_nodes = result.node_count();
        let cur_retired = result.retired_marginal_slots();

        let rescan = (fold_end || self.fold_at_root) && !self.skip_content_twins && self.fold_paid_for_scan(eng, result);
        let action = if rescan { MinimizeAction::Full } else { self.decide(cur_nodes, cur_retired) };
        match action {
            MinimizeAction::Full => {
                let probed = rescan && self.fold_at_root && !fold_end;
                let mut rescan = rescan;
                if probed {
                    let (pairs, work) = (result.pair_count(), eng.limits().work_units().saturating_sub(self.fold_start_work));
                    eng.reduce(result, full_reduction(true, None))?;
                    rescan = self.root_scan_pays(work, pairs, result.pair_count());
                }
                let small_sum = summed_out && cur_nodes <= MID_FOLD_SCAN_MAX_NODES;
                let skip = self.skip_content_twins || !(fold_end || rescan || small_sum);
                let schedule = (!rescan).then_some(&mut self.content_twin_schedule);
                if !(probed && skip) {
                    eng.reduce(result, full_reduction(skip, schedule))?;
                }
                if skip && summed_out && !self.skip_content_twins && !self.fold_at_root
                    && result.node_count() <= MID_FOLD_TRIAL_MAX_NODES
                    && self.fold_paid_for_scan(eng, result)
                {
                    let mut trial = eng.try_clone(result)?;
                    eng.reduce(&mut trial, full_reduction(false, None))?;
                    if trial.node_count().saturating_mul(4) <= result.node_count() {
                        *result = trial;
                    }
                }
                if rescan {
                    self.fold_start_work = eng.limits().work_units();
                    let removed = cur_nodes.saturating_sub(result.node_count()) as f64;
                    self.scan_backoff = if removed < cur_nodes as f64 * SCAN_MIN_REMOVAL_FRACTION {
                        (self.scan_backoff + 1).min(SCAN_MAX_BACKOFF)
                    } else {
                        0
                    };
                }
                self.record_full(
                    result.node_count(),
                    result.retired_marginal_slots(),
                );
                self.throttle.reopen();
                self.shared.rearm();
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
                if self.walk_skip_remaining > 0
                    && (self.walk_skip_min_nodes..=self.walk_skip_max_nodes).contains(&cur_nodes)
                {
                    self.walk_skip_remaining -= 1;
                    return Ok(());
                }
                let skip = if self.throttle.warm { self.throttle.try_skip() } else { self.shared.try_skip() };
                if !skip {
                    self.run_contract_and_record(eng, result, cur_nodes)?;
                }
            }
        }
        Ok(())
    }

    /// Mark the start of a fold for [`Self::fold_paid_for_scan`].
    pub(crate) fn start_fold(&mut self, eng: &Engine) {
        self.fold_start_work = eng.limits().work_units();
    }

    /// Whether the fold ending in `result` spent enough work per pair of it,
    /// counting [`SCAN_FIXED_COST_PAIRS`] more, that a content-twin scan is
    /// cheap beside it. Summing out inside a fold is what leaves count nodes
    /// with equal contents, and such a fold does much work per pair it leaves;
    /// a fold of one merge that only grows the diagram does little and leaves
    /// few twins a later scan would not find anyway. The bar doubles after
    /// each forced scan in a row that removed too little
    /// ([`SCAN_MIN_REMOVAL_FRACTION`]).
    fn fold_paid_for_scan(&self, eng: &Engine, result: &Tdd) -> bool {
        result.has_marginal_level()
            && eng.limits().work_units().saturating_sub(self.fold_start_work)
                >= (SCAN_FOLD_WORK_PER_PAIR << self.scan_backoff)
                    .saturating_mul(result.pair_count() as u64 + SCAN_FIXED_COST_PAIRS)
    }

    /// Whether a scan inside the root's fold pays, after a prune and
    /// contraction took a merge's result from `before` pairs to `after`. No
    /// later fold reads the root's diagram, so the scan saves only on the
    /// fold's later conjunctions. Taking the share of pairs the contraction
    /// removed as the share the scan would remove, and the fold's work so far
    /// (`work`) as what its later conjunctions do, the saving must cover what
    /// [`Self::fold_paid_for_scan`] charges for a scan of `after` pairs.
    fn root_scan_pays(&self, work: u64, before: usize, after: usize) -> bool {
        let removed = before.saturating_sub(after) as u128;
        let bar = u128::from(SCAN_FOLD_WORK_PER_PAIR << self.scan_backoff);
        removed * u128::from(work) >= bar * (after as u128 + u128::from(SCAN_FIXED_COST_PAIRS)) * before as u128
    }

    /// Run twin contraction alone and record the fraction of nodes it
    /// removed.
    fn run_contract_and_record(
        &mut self, eng: &Engine, result: &mut Tdd,
        pre_nodes: usize,
    ) -> Result<(), OperationError> {
        let work_before = eng.limits().work_units();
        minimize_contract_only(eng, result)?;
        let work = eng.limits().work_units().saturating_sub(work_before);
        let post_nodes = result.node_count();
        let removed = pre_nodes.saturating_sub(post_nodes);
        let fraction = if pre_nodes == 0 { 0.0 } else { removed as f64 / pre_nodes as f64 };
        if fraction < CONTRACT_MIN_REMOVAL_FRACTION && work as f64 > CONTRACT_WALK_WORK_PER_NODE * pre_nodes as f64 {
            self.walk_skip_remaining = CONTRACT_SKIP_COUNT;
            self.walk_skip_min_nodes = pre_nodes / 4;
            self.walk_skip_max_nodes = pre_nodes.saturating_mul(4);
        }
        self.throttle.record(fraction);
        self.shared.record(fraction);
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/minimize.rs"]
mod tests;
