//! One pass: count the diagram's nodes, encode it into the solver, probe the
//! encoded nodes through the library's [`Tdd::probe_nodes`], and filter the
//! dead nodes out of the diagram. The compile step calls
//! [`SatPruneCtx::probe_tdd`].

use tididi::Engine;
use tididi::OperationError;
use std::time::Instant;

use super::ctx::{PassSolver, SatPruneCtx};
use crate::compile::minimize::full_reduction;
use tididi::apply::FilterOutcome;
use tididi::cnf::{ProbeOrder, ProbeOutcome, Witness};
use tididi::diagram::Tdd;

/// Outcome of a single triggered pruning pass.
///
/// `Rewritten` carries the diagram the caller swaps into the compile slot;
/// the diagram probed is never modified.
#[derive(Debug)]
pub(crate) enum PruneOutcome {
    /// Nothing to swap in: no node was proved dead, or the pass stopped
    /// before a rewrite finished (the wall, a failed count or encode, a
    /// rewrite over a limit).
    NoOp,
    /// Bottom-up propagation reached the root: replace the slot with
    /// `Tdd::zero(vtree)`.
    DeadRoot,
    /// Some interior nodes were dead and the rewrite produced a new TDD.
    /// Caller swaps `tdds[t.idx()] = Some(new_tdd)`.
    Rewritten(Tdd),
}

impl SatPruneCtx {
    /// The rewrite step: the library's node filter drops every node the probe
    /// marked dead, every pair that reaches one (or the `ZERO` sentinel, or an
    /// inline count of zero) and every node those drops empty, then runs the
    /// compiler's full reduction on what is left.
    ///
    /// With no node marked dead (every probe returned SAT or unknown) the
    /// pass has nothing to act on: skip the rewrite and bump the consec-zero
    /// counter (adaptive disable below).
    fn apply_rewrite_step(&mut self, eng: &Engine, tdd: &Tdd, probe: &ProbeOutcome) -> PruneOutcome {
        if probe.stats().dead == 0 {
            self.consec_zero_passes += 1;
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
                self.consec_zero_passes += 1;
                return PruneOutcome::NoOp;
            }
        };
        let pairs_dropped = removed.pairs_dropped;
        self.stats.pairs_filtered += pairs_dropped;
        self.stats.rewrite_propagated_kills += removed.emptied_nodes;
        self.stats.rewrite_us += rewrite_start.elapsed().as_micros() as u64;
        self.stats.passes_rewritten += 1;
        if new_tdd.is_zero() {
            // Rewrite collapsed everything (cascade caught what propagation
            // missed in pathological orderings). Treat as DeadRoot.
            self.stats.passes_dead_root += 1;
            return PruneOutcome::DeadRoot;
        }
        // A rewrite that dropped at least `wall_reset_drop_pct`% of the
        // diagram resets the wall meter (see `wall_exempt_us`).
        let post_size = new_tdd.pair_count() as u64;
        if pairs_dropped * 100
            >= self.wall_reset_drop_pct.saturating_mul(pairs_dropped + post_size)
        {
            self.wall_exempt_us = self.stats.spend_us();
        }
        PruneOutcome::Rewritten(new_tdd)
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

    /// Run one full sat-prune trigger pass on `tdd` (see module doc for the
    /// SAT-based soundness model). Caller substitutes `Tdd::zero(vtree)` for
    /// the slot when `DeadRoot` is returned.
    ///
    /// # Pass anatomy
    ///
    /// 1. **Cheap exits** — a zero root → `DeadRoot`; wall-time ceiling
    ///    tripped → sticky-disable and bail.
    /// 2. **Encode** — Tseitin-encode reachable internal nodes into the
    ///    long-lived CaDiCaL instance behind an activation literal `a`. Per-pass
    ///    clauses are gated so they can be deactivated at end-of-pass with a
    ///    single unit add.
    /// 3. **Probe** — the library's [`Tdd::probe_nodes`] marks the nodes of
    ///    count 0 dead, runs up to `allsat_k` enumeration rounds, then walks
    ///    the levels bottom-up and, for each reachable internal node `n` not
    ///    yet decided, calls `assume(y_n) ∧ solve()` with a per-probe conflict
    ///    budget. UNSAT marks `n` dead; unknown treats it as live (we never
    ///    kill on uncertainty). Bottom-up propagation lets a dead child kill
    ///    its parent without a fresh SAT call. `PassSolver` answers the
    ///    solves and stops the probe at the pass budget, the probe cap or the
    ///    running-rate gate.
    /// 4. **End-of-pass** — gate `a` off so the per-pass Tseitin clauses don't
    ///    accumulate across triggers; original CNF clauses stay active.
    /// 5. **Rewrite** — if the root ended up dead, return
    ///    [`PruneOutcome::DeadRoot`] (caller substitutes `Tdd::zero`).
    ///    Otherwise filter the dead nodes and every pair that reaches one
    ///    out of the diagram, then reduce it fully to canonicalize:
    ///    [`PruneOutcome::Rewritten`] when any pair was dropped, else
    ///    [`PruneOutcome::NoOp`].
    ///
    /// Updates all probe-related counters in `self.stats`.
    pub(crate) fn probe_tdd(&mut self, eng: &Engine, tdd: &Tdd, tdd_size: usize) -> PruneOutcome {
        // Zero sentinel: trivially dead — max success, reset adaptive gate.
        if tdd.is_zero() {
            self.stats.passes_dead_root += 1;
            self.consec_zero_passes = 0;
            return PruneOutcome::DeadRoot;
        }

        self.stats.passes_triggered += 1;
        self.stats.nodes_inspected += tdd_size as u64;
        self.last_trigger_size = tdd_size;

        // Wall-time ceiling on the spend since the last productive rewrite
        // (see `wall_exempt_us`). Once it is hit, pruning stops for the rest
        // of this compile so the compiler keeps its remaining wall.
        if self.disabled_by.is_none() {
            let total_us = self.stats.spend_us();
            let burn_us = total_us.saturating_sub(self.wall_exempt_us);
            if burn_us >= self.wall_ms * 1000 {
                self.disabled_by = Some("time-share");
                self.consec_zero_passes += 1;
                return PruneOutcome::NoOp;
            }
        }

        // If every internal level is marginal, there are no nodes anywhere to
        // probe — skip and bump consec_zero so the adaptive gate eventually
        // disables this pass. Otherwise, fall through and run the partial
        // encoding on the non-marginal upper levels.
        let has_non_marginal_internal = tdd
            .vtree()
            .internal_bottomup()
            .any(|(t_int, _, _)| !tdd.levels()[t_int.idx()].is_marginal());
        if !has_non_marginal_internal {
            self.consec_zero_passes += 1;
            return PruneOutcome::NoOp;
        }

        // ── Per-node model counts ──
        // Evaluated once per pass. Used for the MC=0 short-circuit kill
        // before encoding and for ordering the walk within each level.
        let counts = match self.node_counts(eng, tdd) {
            Ok(counts) => counts,
            // No SAT clauses or diagram changes have been made. Leave the
            // optional pruning pass and let the compiler honor its active cut.
            Err(_) => return PruneOutcome::NoOp,
        };

        // ── Encoding ──
        // `None`: the encode budget was exceeded with nothing encoded, or the
        // encoding was refused, and the activation literal is already retired.
        let rounds = self.allsat_k;
        let mut solver = PassSolver::open(self);
        let Some(enc) = solver.encode(tdd) else {
            return PruneOutcome::NoOp;
        };

        // ── Probe ──
        // Every SAT probe's model certifies alive the nodes it makes true,
        // exactly when the enumeration runs.
        let witness = if rounds > 0 { Witness::NodeLiterals } else { Witness::None };
        let probed = tdd.probe_nodes(&enc, ProbeOrder::BottomUp { counts: &counts }, rounds, witness, &mut solver);
        if let Ok(probe) = &probed {
            solver.record(&probe.stats());
        }

        // ── End of pass: retire this pass's gated clauses ──
        solver.retire();
        // Only a fresh variable of 0 or `i32::MIN` fails the probe, and the
        // context's counter gives neither. A failed probe would end the pass
        // as one that cannot finish: `a` retired, nothing to swap in.
        let Ok(probe) = probed else {
            return PruneOutcome::NoOp;
        };

        // Running-rate gate for a walk that ended between two of its checks.
        // The probe counters change only in the walk, so no other site needs it.
        self.maybe_disable_by_running_rate();

        // ── Root-death check after bottom-up propagation ──
        //
        // The probe marks any internal node whose pairs all reference a dead
        // child, the root included. If the root is dead, signal the caller.
        if probe.is_dead(tdd.output()) {
            self.stats.passes_dead_root += 1;
            self.consec_zero_passes = 0;
            return PruneOutcome::DeadRoot;
        }

        // ── Rewrite ──
        self.apply_rewrite_step(eng, tdd, &probe)
    }
}
