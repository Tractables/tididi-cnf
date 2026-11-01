//! One pass: count the diagram's nodes, encode it into the solver, probe the
//! encoded nodes through the library's [`Tdd::probe_nodes`], and filter the
//! dead nodes out of the diagram. The compile step calls
//! [`SatPruneCtx::probe_tdd`].

use std::time::Instant;

use tididi::{Engine, OperationError};
use tididi::apply::FilterOutcome;
use tididi::cnf::{ProbeOrder, ProbeOutcome, Witness};
use tididi::diagram::Tdd;

use crate::compile::minimize::full_reduction;
use super::ctx::{PassSolver, SatPruneCtx};
use super::stats::PruneStop;

/// Outcome of a single triggered pruning pass. The diagram probed is never
/// modified.
#[derive(Debug)]
pub(crate) enum PruneOutcome {
    /// Nothing to swap in: no node was proved dead, or the pass stopped
    /// before a rewrite finished (the spend cap, a failed count or encode, a
    /// rewrite over a limit).
    NoOp,
    /// No model of the formula reaches the root: the caller replaces the
    /// diagram with the false one on its vtree.
    DeadRoot,
    /// Some interior nodes were dead, and this diagram, without them,
    /// replaces the one probed. Boxed: a diagram is far larger than the
    /// other variants.
    Rewritten(Box<Tdd>),
}

impl SatPruneCtx {
    /// The rewrite step: the library's node filter drops every node the probe
    /// marked dead, every pair that reaches one (or the `ZERO` sentinel, or an
    /// inline count of zero) and every node those drops empty, then runs the
    /// compiler's full reduction on what is left.
    ///
    /// With no node marked dead (every probe returned SAT or unknown) the
    /// pass has nothing to act on: it skips the rewrite and counts as one
    /// that removed nothing.
    fn apply_rewrite_step(&mut self, eng: &Engine, tdd: &Tdd, probe: &ProbeOutcome) -> PruneOutcome {
        if probe.stats().dead == 0 {
            self.record_empty_pass();
            return PruneOutcome::NoOp;
        }
        self.consec_zero_passes = 0;
        let rewrite_start = Instant::now();
        // The filter asks about exactly the stored structural internal nodes,
        // the only nodes the probe marks.
        let keep = |id| !probe.is_dead(id);
        let (new_tdd, removed) = match eng.filter_nodes_with(tdd, keep, full_reduction(self.skip_content_twins, None)) {
            Ok(FilterOutcome::Filtered { tdd, stats } | FilterOutcome::Unsatisfiable { tdd, stats }) => (tdd, stats),
            // Pruning is optional, so a filter or reduction that hits a limit
            // skips this prune and keeps the original TDD. That is sound for
            // counting: every pair it would drop reaches a SAT-proved-dead
            // child. A marked node is a reachable internal one, so the filter
            // always rejects a node and `Unchanged` does not arise; it would
            // mean the original TDD is the answer, so it takes the same arm.
            Ok(FilterOutcome::Unchanged) | Err(_) => {
                self.record_empty_pass();
                return PruneOutcome::NoOp;
            }
        };
        let pairs_dropped = removed.pairs_dropped;
        self.stats.pairs_filtered += pairs_dropped;
        self.stats.rewrite_propagated_kills += removed.emptied_nodes;
        self.stats.rewrite_us += rewrite_start.elapsed().as_micros() as u64;
        self.stats.passes_rewritten += 1;
        if new_tdd.is_zero() {
            // The filter's emptied nodes reached the root, which the probe's
            // own propagation did not.
            self.stats.passes_dead_root += 1;
            return PruneOutcome::DeadRoot;
        }
        // A rewrite that dropped at least `reset_drop_pct` percent of the
        // diagram is productive (see `spend_exempt_us`).
        let post_size = new_tdd.pair_count() as u64;
        if pairs_dropped * 100 >= self.limits.reset_drop_pct.saturating_mul(pairs_dropped + post_size) {
            self.spend_exempt_us = self.stats.spend_us();
        }
        PruneOutcome::Rewritten(Box::new(new_tdd))
    }

    /// Per-node model counts of `tdd`, saturating at `u128::MAX`. The pass
    /// reads them only for ordering and for exact-zero kills, neither of
    /// which needs an overflowed node's exact value; zero stays exact. A
    /// refused fold returns the batch error.
    fn node_counts(
        &mut self,
        eng: &Engine,
        tdd: &Tdd,
    ) -> Result<Vec<Vec<u128>>, OperationError> {
        let t0 = Instant::now();
        let counts = eng.node_counts_u128(tdd);
        self.stats.count_order_us += t0.elapsed().as_micros() as u64;
        counts
    }

    /// Encode `tdd` into a scoped solver pass, probe its nodes bottom-up by
    /// `counts`, and retire the pass's activation literal before returning.
    fn encode_and_probe(&mut self, tdd: &Tdd, counts: &[Vec<u128>]) -> Option<ProbeOutcome> {
        let rounds = self.limits.allsat_k;
        let mut solver = PassSolver::open(self);
        let enc = solver.encode(tdd)?;
        let witness = if rounds > 0 { Witness::NodeLiterals } else { Witness::None };
        let probed = tdd.probe_nodes(&enc, ProbeOrder::BottomUp { counts }, rounds, witness, &mut solver);
        if let Ok(probe) = &probed {
            solver.record(&probe.stats());
        }
        solver.retire();
        probed.ok()
    }

    /// Run one pass on `tdd`, a diagram of `tdd_size` pairs, as the module
    /// doc describes, and say what replaces it. Adds what the pass did to
    /// the context's tallies, and stops pruning for the compile when one of
    /// its gates says so.
    pub(crate) fn probe_tdd(&mut self, eng: &Engine, tdd: &Tdd, tdd_size: usize) -> PruneOutcome {
        // A conjunction may keep unreachable nodes (see `tididi::and`), so a
        // false diagram can still be large enough to fire a pass.
        if tdd.is_zero() {
            self.stats.passes_dead_root += 1;
            self.consec_zero_passes = 0;
            return PruneOutcome::DeadRoot;
        }

        self.stats.passes_triggered += 1;
        self.stats.pairs_inspected += tdd_size as u64;
        self.last_trigger_size = tdd_size;

        // The spend caps on the work since the last productive rewrite (see
        // `spend_exempt_us`). Once either is reached, pruning stops for the
        // rest of this compile so the compiler keeps its remaining time.
        if self.spend_cap_reached() {
            self.stats.disabled_by = Some(PruneStop::TimeShare);
            self.record_empty_pass();
            return PruneOutcome::NoOp;
        }

        let has_non_marginal_internal = tdd
            .vtree()
            .internal_bottomup()
            .any(|(t_int, _, _)| !tdd.levels()[t_int.idx()].is_marginal());
        if !has_non_marginal_internal {
            self.record_empty_pass();
            return PruneOutcome::NoOp;
        }

        let Ok(counts) = self.node_counts(eng, tdd) else {
            return PruneOutcome::NoOp;
        };
        let Some(probe) = self.encode_and_probe(tdd, &counts) else {
            return PruneOutcome::NoOp;
        };

        // The kill-rate gate, for a walk that ended between two of its checks.
        self.maybe_disable_by_running_rate();

        if probe.is_dead(tdd.output()) {
            self.stats.passes_dead_root += 1;
            self.consec_zero_passes = 0;
            return PruneOutcome::DeadRoot;
        }

        self.apply_rewrite_step(eng, tdd, &probe)
    }
}

#[cfg(test)]
#[path = "tests/probe.rs"]
mod tests;
