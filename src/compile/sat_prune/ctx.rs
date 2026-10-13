//! The sat-prune pass's solver context: the incremental CaDiCaL instance, its
//! clause seeding, and the solver one pass's encoding and probe run on, with
//! the bounded solve every probe goes through and the rules that stop a probe.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use tididi::cnf::{
    ClauseSink, CnfEncoding, CnfScheme, EncodePoint, ProbePoint, ProbePolicy, ProbeStats,
    SatOracle, SolveCall, SolveStatus,
};
use tididi::diagram::Tdd;
use tididi::vtree::VarId;
use vitri::sat::{Bounded, CaDiCal, Status, WallClockTerminator};

use vitri::cnf::CnfFormula;

use crate::budget::resolve_scaled;

use super::stats::SatPruneStats;

/// The pair count at which a just-built diagram first triggers a pass.
const SAT_PRUNE_THRESHOLD: usize = 10000;

/// Conflict budget per AllSAT prelude solve.
pub(super) const ALLSAT_CONFLICT_BUDGET: i32 = 50;

/// Hard cap on probes per trigger pass.
pub(super) const MAX_PROBES_PER_PASS: u64 = 2_000;

/// Hard upper bound on partial-TDD size at which we even attempt
/// a probe pass. Above this, the Tseitin encoding alone is
/// prohibitive (millions of clauses per pass) and the CaDiCaL
/// state accumulates across passes, crowding the apply arenas.
pub(super) const MAX_NODES: usize = 2_000_000;

/// Growth multiplier above which the next pass is allowed to fire.
/// `next_allowed_size = max(threshold, last_trigger_size * GROWTH_FACTOR)`.
pub(super) const GROWTH_FACTOR: f32 = 1.1;

/// Threshold for the consecutive-zero-pass disable.
pub(super) const MAX_CONSEC_ZERO_PASSES: u32 = 2;

/// Minimum cumulative kill rate (probes_unsat / probes_attempted) below
/// which pruning is disabled. Only checked once `probes_attempted` meets
/// `MIN_PROBES_FOR_RATE_CHECK`.
pub(super) const MIN_RUNNING_KILL_RATE: f32 = 0.05;

/// Probe-count threshold for the running-rate gate. Below this, the gate
/// is skipped (not enough data to make a decision).
pub(super) const MIN_PROBES_FOR_RATE_CHECK: u64 = 30;

/// Long-lived SAT-pruning context: one incremental CaDiCaL solver loaded
/// with the full original CNF, plus prune statistics.
pub(crate) struct SatPruneCtx {
    pub(super) solver: CaDiCal,
    /// When the current pass's budget runs out, set at pass start before any
    /// solve.
    ///
    /// The terminator itself is not a field: a `vitri::sat::Bounded` guard
    /// borrows the solver, so a struct holding both would be self-referential.
    /// `PassSolver`'s solve builds one per solve from this instead.
    pub(super) pass_end: Instant,
    /// One-past-the-last DIMACS variable currently in use. Original CNF
    /// uses 1..=num_vars; Tseitin auxiliaries grow above this and are
    /// retired (via activation-literal disable) at end-of-pass.
    pub(super) next_var: i32,
    /// Conflict budget per node probe (CaDiCaL `limit("conflicts", N)`).
    pub(super) probe_conflict_budget: i32,
    /// How far past the pass budget one in-flight solve may run before the
    /// pass terminator stops it (milliseconds).
    pub(super) solve_overrun_ms: u64,
    /// Wall-time cap for the Tseitin encoding phase itself (milliseconds).
    /// Pathological CNFs can spend the whole
    /// per-CNF compile budget building the encoding before any probing
    /// happens. When the encode loop exceeds this cap, the fully-encoded
    /// bottom-up prefix is probed, or, when nothing was encoded, the pass is
    /// retired. 1/60 of the run wall clamped to [2 s, 60 s]; 2 s when the run
    /// has no wall.
    pub(super) max_encode_ms: u64,
    /// Deterministic encode cut for tests: stop emitting after this many
    /// internal levels (checked alongside `max_encode_ms`). `usize::MAX` in
    /// production.
    pub(super) max_encode_levels: usize,
    /// Size at which we last fired a probe pass (see `should_fire`).
    pub(super) last_trigger_size: usize,
    /// Adaptive-disable signal: count of consecutive `probe_tdd` passes that
    /// produced zero kills (no SAT-UNSAT and no transitively-dead nodes). Once
    /// this hits `MAX_CONSEC_ZERO_PASSES`, pruning is disabled for the rest
    /// of this compile — `should_fire` returns false thereafter. Reset to 0
    /// every time a pass produces at least one kill.
    pub(super) consec_zero_passes: u32,
    /// Why pruning is off for the rest of this compile, once the wall
    /// (`"time-share"`) or the running-rate gate (`"low-kill-rate"`)
    /// turns it off. `should_fire` returns false while it is set.
    pub(super) disabled_by: Option<&'static str>,
    /// AllSAT-style enumeration. When > 0, before the main probe
    /// loop, issue up to `allsat_k` unconditioned global solves under
    /// `assume={a, a_block}` (no `y_n` target). Each SAT model harvests
    /// aliveness witnesses for every reachable internal y-literal; a blocking
    /// clause `¬a_block ∨ ¬y_{w1} ∨ ¬y_{w2} ∨ …` forces the next model to
    /// differ on at least one witnessed literal, and `a_block` is retired
    /// before the probes. If a round returns UNSAT, all distinct
    /// alive-signatures have been enumerated → every unwitnessed reachable
    /// internal node is **definitively dead**. With it on, every
    /// SAT probe's model also certifies the nodes it makes true alive for the
    /// rest of the pass. `0` = disabled (tests only).
    pub(super) allsat_k: u32,
    /// Wall-clock ceiling on unproductive sat-prune work (probe + encode +
    /// rewrite + model counts since the last productive rewrite), in
    /// milliseconds, and the budget of one pass. Once reached, pruning
    /// sticky-disables for the compile. 1/120 of the run wall clamped to
    /// [1 s, 30 s]; 1 s when the run has no wall.
    pub(super) wall_ms: u64,
    /// Spend-meter snapshot (probe+encode+rewrite+count µs) taken at
    /// the last productive rewrite (drop fraction ≥ `wall_reset_drop_pct`).
    /// The wall compares spend since this snapshot, so the ceiling caps
    /// consecutive unproductive work, not total work. Without it, the wall
    /// sticky-disables right after the most productive passes, which are
    /// also the most expensive ones.
    pub(super) wall_exempt_us: u64,
    /// Minimum drop percentage — pairs_dropped / (pairs_dropped + post-
    /// rewrite size) × 100 — for a rewrite to reset the wall meter.
    pub(super) wall_reset_drop_pct: u64,

    /// The tallies this context has accumulated so far.
    pub(super) stats: SatPruneStats,
    /// Whether the full reduction after a rewrite skips the content-twin
    /// scan, as a recovery cofactor's compile does.
    pub(super) skip_content_twins: bool,
}

impl SatPruneCtx {
    /// Build the context: a fresh CaDiCaL instance loaded with `formula`'s
    /// clauses, not yet solved. `wall_ms` is the run's wall, which the time
    /// knobs scale with; `skip_content_twins` is the compile's reduction
    /// policy.
    ///
    /// `None` when no solver could be built: there is nothing to prune with,
    /// and the caller compiles without the SAT-prune probe (which only ever
    /// shrinks the diagram, never changes the count).
    pub(crate) fn new(formula: &CnfFormula, wall_ms: Option<u64>, skip_content_twins: bool) -> Option<Self> {
        let start = Instant::now();

        let mut solver = CaDiCal::new()?;
        // Reserve room so CaDiCaL pre-allocates its variable tables.
        let num_orig_vars = formula.num_vars();
        if num_orig_vars > 0 {
            solver.reserve(num_orig_vars as i32);
        }
        for clause in formula.clauses() {
            for lit in &clause.literals {
                solver.add(lit.to_dimacs());
            }
            solver.add(0);
        }
        let next_var = num_orig_vars as i32 + 1;
        let solver_setup_us = start.elapsed().as_micros() as u64;

        // Time knobs scale with the run wall (see the field docs). Each ratio
        // gives the knob's fixed value at a 120 s wall, where the defaults
        // were calibrated, and that value is also its floor, so only longer
        // runs get larger caps.
        let max_encode_ms = resolve_scaled(wall_ms, 1.0 / 60.0, 2000, 2_000, 60_000);
        let wall_ms = resolve_scaled(wall_ms, 1.0 / 120.0, 1_000, 1_000, 30_000);

        Some(SatPruneCtx {
            solver,
            pass_end: start,
            next_var,
            probe_conflict_budget: 5_000,
            solve_overrun_ms: 200,
            max_encode_ms,
            max_encode_levels: usize::MAX,
            last_trigger_size: 0,
            consec_zero_passes: 0,
            disabled_by: None,
            allsat_k: 4,
            wall_ms,
            wall_exempt_us: 0,
            wall_reset_drop_pct: 50,
            stats: SatPruneStats {
                solver_setup_us,
                ..SatPruneStats::default()
            },
            skip_content_twins,
        })
    }

    /// Whether the current pass has spent its budget.
    fn pass_over(&self) -> bool {
        Instant::now() >= self.pass_end
    }

    /// Check whether the size threshold (`SAT_PRUNE_THRESHOLD`) + exponential
    /// gate allow a probe pass to fire now. Caller passes the current
    /// partial-TDD size; if this returns true, the caller should proceed to
    /// `probe_tdd`. Otherwise the pass is suppressed (no SAT cost).
    ///
    /// The exponential gate ensures that after a pass at size `s`, the next
    /// pass doesn't fire until the partial TDD reaches `s * growth_factor`.
    /// This caps the per-compile pass count at `O(log_growth(peak /
    /// threshold))`. Without the gate, the trigger fires on every post-batch
    /// step whose size exceeds the threshold (potentially hundreds of times
    /// per compile), which dominates compile time even on a fast probe loop.
    ///
    /// Pruning also stops for the compile after `MAX_CONSEC_ZERO_PASSES`
    /// passes in a row that killed nothing.
    pub(crate) fn should_fire(&self, current_size: usize) -> bool {
        if self.disabled_by.is_some() || self.consec_zero_passes >= MAX_CONSEC_ZERO_PASSES {
            return false;
        }
        // Skip when the partial TDD is already too large for the
        // Tseitin encoding to be cost-effective. The encoded clause
        // count scales with reachable internal pairs; above
        // `MAX_NODES` the encode-time budget would always fire and
        // the accumulated CaDiCaL state would crowd apply memory.
        if current_size > MAX_NODES {
            return false;
        }
        let min_size_for_next = (self.last_trigger_size as f32 * GROWTH_FACTOR) as usize;
        current_size >= min_size_for_next.max(SAT_PRUNE_THRESHOLD)
    }

    /// Cumulative-kill-rate gate. If we've issued at least
    /// `MIN_PROBES_FOR_RATE_CHECK` probes and the cumulative `unsat / attempts`
    /// rate is below `MIN_RUNNING_KILL_RATE`, sticky-disable for the rest of
    /// this compile. Returns whether pruning is disabled after the check.
    ///
    /// Why this matters: consec-zero only catches passes with *no* kills;
    /// CNFs with one kill per pass keep
    /// triggering passes and paying SAT cost for negligible benefit. This gate
    /// catches the steady-but-low-yield case.
    ///
    /// Evaluation schedule: every `MIN_PROBES_FOR_RATE_CHECK` probes *inside*
    /// the walk (the probe's `Probed` point, see `PassSolver`) and once more
    /// at the end of every non-fast-path `probe_tdd` exit. The probe minimum is an evidence floor,
    /// not a check schedule — a walk whose kill rate is already hopeless exits
    /// at its first decision point instead of burning the rest of the pass
    /// budget to reach the same verdict.
    pub(super) fn maybe_disable_by_running_rate(&mut self) -> bool {
        if self.disabled_by.is_some() {
            return true;
        }
        if self.stats.probes_attempted < MIN_PROBES_FOR_RATE_CHECK {
            return false;
        }
        let rate = self.stats.probes_unsat as f32 / self.stats.probes_attempted as f32;
        if rate < MIN_RUNNING_KILL_RATE {
            self.disabled_by = Some("low-kill-rate");
        }
        self.disabled_by.is_some()
    }

    /// Return a snapshot of the current stats, including the derived
    /// `disabled_by` field that captures *why* SAT-prune was disabled
    /// (if at all). Intended for post-compile collection by the binary.
    pub(crate) fn stats_snapshot(&self) -> SatPruneStats {
        let disabled_by = self.disabled_by.unwrap_or(
            if self.consec_zero_passes >= MAX_CONSEC_ZERO_PASSES { "empty-passes" } else { "" },
        );
        SatPruneStats {
            disabled_by,
            ..self.stats.clone()
        }
    }

    /// Allocate the next fresh DIMACS variable.
    #[inline]
    fn fresh_var(&mut self) -> i32 {
        let v = self.next_var;
        self.next_var += 1;
        v
    }
}

/// The solver as one pass uses it, from its encoding to the end of its
/// probe: variables from the context's counter, each diagram variable as its
/// own DIMACS literal, every clause of the encoding gated on `¬a`, solves
/// bounded by the pass, and the rules that stop the probe.
pub(super) struct PassSolver<'a> {
    ctx: &'a mut SatPruneCtx,
    /// The pass's activation literal.
    a: i32,
    /// When the pass opened; `max_encode_ms` counts from here.
    start: Instant,
    /// The tallies as the pass opened: the probe's counts add to these.
    base: SatPruneStats,
}

impl<'a> PassSolver<'a> {
    /// Open a pass on `ctx`: start the encode clock, then allocate the
    /// activation literal `a`.
    pub(super) fn open(ctx: &'a mut SatPruneCtx) -> Self {
        let base = ctx.stats.clone();
        let start = Instant::now();
        let a = ctx.fresh_var();
        PassSolver { ctx, a, start, base }
    }

    /// Encode all reachable internal nodes of `tdd` into the long-lived
    /// CaDiCaL instance under the activation literal `a`, through the
    /// library's [`Tdd::encode_cnf`]: each node's literal is equivalent to its
    /// function, with a marginal child of positive count read as ⊤ and one of
    /// count 0 as ⊥.
    ///
    /// When the encode budget (`max_encode_ms`, `max_encode_levels`) runs out
    /// after some nodes were defined, the fully defined bottom-up prefix is
    /// kept and the nodes above it have no literal. Returns `None` when it
    /// runs out before any node was defined, or when the library refuses the
    /// encoding; the activation literal is then retired (`¬a` unit added) so
    /// every gated clause emitted so far is trivially satisfied,
    /// `passes_aborted_encode_budget` is incremented, and
    /// `consec_zero_passes` is bumped — the caller returns
    /// `PruneOutcome::NoOp`.
    pub(super) fn encode(&mut self, tdd: &Tdd) -> Option<CnfEncoding> {
        let encoding = tdd.encode_cnf(CnfScheme::Equivalence, self.a, self);
        self.ctx.stats.encode_us += self.start.elapsed().as_micros() as u64;
        match encoding {
            Ok(enc) if enc.stop().is_none() => Some(enc),
            // Keep the fully encoded bottom-up prefix and probe it instead of
            // discarding the pass. Sound: every node before the stop point
            // has its complete biconditional definition (definitions only
            // reference lower levels), so UNSAT of Φ ∧ a ∧ y_n still
            // certifies deadness. The nodes from the stop point on have no
            // literal, so the probe treats them exactly like unreachable
            // nodes.
            Ok(enc) if enc.encoded_nodes() > 0 => {
                self.ctx.stats.passes_encode_prefix += 1;
                self.ctx.stats.prefix_nodes_skipped += enc.skipped();
                Some(enc)
            }
            // Nothing encoded, or the library refused the encoding: retire
            // activation `a` so every gated clause emitted so far is
            // satisfied trivially (¬a ∨ …), and count the pass as a zero
            // pass. No refusal arises here: every literal the sink gives is
            // positive, a weighted compile builds no pruning context, and
            // this call runs without allocation limits.
            _ => {
                self.retire();
                self.ctx.stats.passes_aborted_encode_budget += 1;
                self.ctx.consec_zero_passes += 1;
                None
            }
        }
    }

    /// Force `¬a` true. Every clause of the pass's encoding contains `¬a`, so
    /// all of them are satisfied and inactive from now on.
    pub(super) fn retire(&mut self) {
        self.ctx.solver.add(-self.a);
        self.ctx.solver.add(0);
    }

    /// Set the tallies the probe keeps to the pass's base plus `probe`'s
    /// counts, so the running-rate gate reads every probe so far.
    pub(super) fn record(&mut self, probe: &ProbeStats) {
        let (stats, base) = (&mut self.ctx.stats, &self.base);
        stats.probes_attempted = base.probes_attempted + probe.probes;
        stats.probes_unsat = base.probes_unsat + probe.probes_unsat;
        stats.probes_unknown = base.probes_unknown + probe.probes_unknown;
        stats.nodes_pruned = base.nodes_pruned + probe.dead;
        stats.zero_count_kills = base.zero_count_kills + probe.zero_count_dead;
        stats.allsat_kills = base.allsat_kills + probe.enumeration_dead;
        stats.allsat_global_attempted = base.allsat_global_attempted + probe.rounds;
        stats.allsat_global_sat = base.allsat_global_sat + probe.rounds_sat;
        stats.allsat_global_unsat = base.allsat_global_unsat + probe.rounds_unsat;
        stats.allsat_global_unknown = base.allsat_global_unknown + probe.rounds_unknown;
        stats.allsat_witnesses_harvested = base.allsat_witnesses_harvested + probe.round_witnesses;
        stats.nodes_skipped_witness = base.nodes_skipped_witness + probe.witness_skips;
        stats.witness_val_queries = base.witness_val_queries + probe.value_reads;
        stats.dead_child_reorders = base.dead_child_reorders + probe.reordered_levels;
    }
}

impl ClauseSink for PassSolver<'_> {
    fn fresh_var(&mut self) -> i32 {
        self.ctx.fresh_var()
    }

    fn leaf_literal(&mut self, var: VarId) -> i32 {
        // DIMACS encoding: VarId(v) -> v positive, -v negative.
        var.0 as i32
    }

    /// Add the clause `body ∨ ¬a`.
    fn clause(&mut self, body: &[i32]) {
        for &lit in body {
            self.ctx.solver.add(lit);
        }
        self.ctx.solver.add(-self.a);
        self.ctx.solver.add(0);
    }

    /// Stop before a level once `max_encode_levels` levels are done, and
    /// before a level or every 1,024th node of one once `max_encode_ms` has
    /// passed: a single huge level can dominate the encode cost.
    fn poll(&mut self, at: EncodePoint) -> ControlFlow<()> {
        let out_of_levels =
            matches!(at, EncodePoint::Level { complete, .. } if complete >= self.ctx.max_encode_levels);
        if out_of_levels || self.start.elapsed().as_millis() as u64 >= self.ctx.max_encode_ms {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

impl SatOracle for PassSolver<'_> {
    fn add_clause(&mut self, clause: &[i32]) {
        for &lit in clause {
            self.ctx.solver.add(lit);
        }
        self.ctx.solver.add(0);
    }

    /// One `solve()` under `assumptions` and a conflict limit, 50 for an
    /// enumeration round and `probe_conflict_budget` for a probe, bounded by
    /// the pass and counted in `sat_calls` and `probe_us` (a round also in
    /// `allsat_us`). The one solve site for the whole prune context, so no probe
    /// can outrun the pass wall or go unbilled.
    ///
    /// The policy checks the budget between solves; the terminator stops one
    /// solve `solve_overrun_ms` past it. CaDiCaL polls it during search and
    /// inprocessing, so an inprocessing-heavy solve cannot run orders of
    /// magnitude past the budget. The guard owns the terminator, which fires
    /// once the clock reaches that instant, and disconnects it when it drops
    /// at the end of this call.
    fn solve(&mut self, assumptions: &[i32], call: SolveCall) -> SolveStatus {
        let ctx = &mut *self.ctx;
        let round = matches!(call, SolveCall::Round { .. });
        ctx.solver.limit(c"conflicts", if round { ALLSAT_CONFLICT_BUDGET } else { ctx.probe_conflict_budget });
        for &literal in assumptions {
            ctx.solver.assume(literal);
        }
        let terminator = WallClockTerminator::new(Duration::ZERO);
        terminator.deadline_handle().set(ctx.pass_end + Duration::from_millis(ctx.solve_overrun_ms));
        let start = Instant::now();
        let status = Bounded::new(&mut ctx.solver, terminator).solve();
        let us = start.elapsed().as_micros() as u64;
        ctx.stats.sat_calls += 1;
        ctx.stats.probe_us += us;
        if round {
            ctx.stats.allsat_us += us;
        }
        match status {
            Status::Satisfiable => SolveStatus::Sat,
            Status::Unsatisfiable => SolveStatus::Unsat,
            Status::Unknown => SolveStatus::Unknown,
        }
    }

    fn value(&mut self, literal: i32) -> bool {
        self.ctx.solver.val(literal) > 0
    }
}

impl ProbePolicy for PassSolver<'_> {
    /// Set the pass budget before the first solve; stop the enumeration and
    /// the walk once it is spent, and the walk at `MAX_PROBES_PER_PASS`
    /// probes or when the running-rate gate fires. Kills recorded before a
    /// stop still go through the rewrite.
    fn poll(&mut self, at: ProbePoint, stats: &ProbeStats) -> ControlFlow<()> {
        let stop = match at {
            // One pass may spend the whole sat-prune wall, and no more,
            // before the running-rate gate gets to fire between passes.
            ProbePoint::Begin => {
                self.ctx.pass_end = Instant::now() + Duration::from_millis(self.ctx.wall_ms);
                false
            }
            // Shares the pass budget so the enumeration cannot starve the walk.
            ProbePoint::Round { .. } => self.ctx.pass_over(),
            ProbePoint::Solve { .. } => stats.probes >= MAX_PROBES_PER_PASS || self.ctx.pass_over(),
            // The running-rate gate, on its schedule (see
            // `maybe_disable_by_running_rate`).
            ProbePoint::Probed { .. } => {
                self.record(stats);
                self.ctx.stats.probes_attempted.is_multiple_of(MIN_PROBES_FOR_RATE_CHECK)
                    && self.ctx.maybe_disable_by_running_rate()
            }
            _ => false,
        };
        if stop { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }
}
