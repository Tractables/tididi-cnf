//! The SAT pruning context: the incremental CaDiCaL instance and its clause
//! seeding, the limits a compile's passes run under, and the solver one
//! pass's encoding and probe run on, with the bounded solve every probe goes
//! through and the rules that stop a probe.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use tididi::cnf::{
    ClauseSink, CnfEncoding, CnfScheme, EncodePoint, ProbePoint, ProbePolicy, ProbeStats, SatOracle, SolveCall,
    SolveStatus,
};
use tididi::diagram::Tdd;
use tididi::vtree::VarId;
use vitri::cnf::CnfFormula;
use vitri::sat::{Bounded, CaDiCal, Status, WallClockTerminator};

use crate::budget::resolve_scaled;
use super::stats::{PruneStop, SatPruneStats};

/// The limits a compile's SAT pruning passes run under.
/// [`PruneLimits::for_wall`] sets every one of them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PruneLimits {
    /// The pair count at which a just-built diagram first triggers a pass.
    pub(super) threshold: usize,
    /// The pair count above which no pass fires: the encoding would run to
    /// millions of clauses, and the solver state it leaves behind would
    /// crowd the compile's own memory.
    pub(super) max_pairs: usize,
    /// How much a diagram must grow past the size the last pass fired at
    /// before the next one fires, which keeps the passes of one compile
    /// logarithmic in its peak size.
    pub(super) growth_factor: f32,
    /// How many passes in a row may remove nothing before pruning stops for
    /// the compile.
    pub(super) max_consec_zero_passes: u32,
    /// The share of probes that prove their node dead below which pruning
    /// stops for the compile. Passes that each remove a little keep firing,
    /// which the empty-pass rule does not catch.
    pub(super) min_kill_rate: f32,
    /// How many probes the kill rate is first judged after, and then judged
    /// again every time as many more have run, so a hopeless walk stops at
    /// its first judgement.
    pub(super) min_probes_for_rate_check: u64,
    /// The most probes one pass issues.
    pub(super) max_probes_per_pass: u64,
    /// The conflict budget of one probe's solve.
    pub(super) probe_conflict_budget: i32,
    /// The conflict budget of one enumeration round's solve.
    pub(super) round_conflict_budget: i32,
    /// The enumeration rounds before the probe walk, none at 0. With rounds,
    /// every SAT probe's model also certifies the nodes it makes true alive.
    pub(super) allsat_k: u32,
    /// How far past the pass budget one solve may run before the terminator
    /// stops it, in milliseconds.
    pub(super) solve_overrun_ms: u64,
    /// How long one pass's encoding may take, in milliseconds: on some
    /// formulas the encoding alone would spend the compile's time. A pass
    /// that runs out probes the bottom-up prefix it encoded, or retires when
    /// it encoded nothing.
    pub(super) max_encode_ms: u64,
    /// The most internal levels one pass's encoding defines, under the same
    /// rule as `max_encode_ms`.
    pub(super) max_encode_levels: usize,
    /// The spend, in milliseconds, of solving, encoding, rewriting and
    /// counting since the last productive rewrite at which pruning stops for
    /// the compile; also the budget of one pass.
    pub(super) spend_cap_ms: u64,
    /// The share of the diagram, in percent, a rewrite must remove to count
    /// as productive and reset the spend the cap is measured from.
    pub(super) reset_drop_pct: u64,
}

impl PruneLimits {
    /// The limits for a run whose time limit is `wall_ms`, `None` when it has
    /// none. The encoding's time and the spend cap scale with the time limit,
    /// each never below its value for a run with none; the rest are fixed.
    pub(crate) fn for_wall(wall_ms: Option<u64>) -> Self {
        PruneLimits {
            threshold: 10_000,
            max_pairs: 2_000_000,
            growth_factor: 1.1,
            max_consec_zero_passes: 2,
            min_kill_rate: 0.05,
            min_probes_for_rate_check: 30,
            max_probes_per_pass: 2_000,
            probe_conflict_budget: 5_000,
            round_conflict_budget: 50,
            allsat_k: 4,
            solve_overrun_ms: 200,
            max_encode_ms: resolve_scaled(wall_ms, 1.0 / 60.0, 2_000, 2_000, 60_000),
            max_encode_levels: usize::MAX,
            spend_cap_ms: resolve_scaled(wall_ms, 1.0 / 120.0, 1_000, 1_000, 30_000),
            reset_drop_pct: 50,
        }
    }
}

/// Long-lived SAT pruning context: one incremental CaDiCaL solver loaded
/// with the full original CNF, the limits its passes run under, and what
/// they have done so far.
pub(crate) struct SatPruneCtx {
    pub(super) solver: CaDiCal,
    pub(super) limits: PruneLimits,
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
    /// Size at which we last fired a probe pass (see `should_fire`).
    pub(super) last_trigger_size: usize,
    /// Passes in a row that removed nothing; [`SatPruneCtx::record_empty_pass`]
    /// counts them, and a pass that removes a node resets it.
    pub(super) consec_zero_passes: u32,
    /// The spend, as [`SatPruneStats::spend_us`] measures it, at the last
    /// productive rewrite. The spend cap is measured from here, so it caps
    /// unproductive work, not total work: the most productive passes are
    /// also the most expensive ones.
    pub(super) spend_exempt_us: u64,
    /// The tallies this context has accumulated so far, and why pruning
    /// stopped, once it has: `should_fire` returns false from then on.
    pub(super) stats: SatPruneStats,
    /// Whether the full reduction after a rewrite skips the content-twin
    /// scan; [`CompileRun::cofactor`](crate::compile::CompileRun::cofactor)
    /// says when.
    pub(super) skip_content_twins: bool,
}

impl SatPruneCtx {
    /// Build the context: a fresh CaDiCaL instance loaded with `formula`'s
    /// clauses, not yet solved, whose passes run under `limits`;
    /// `skip_content_twins` is the compile's reduction policy.
    ///
    /// `None` when no solver could be built: there is nothing to prune with,
    /// and the caller compiles without pruning, which only ever shrinks the
    /// diagram, never changes the count.
    pub(crate) fn new(formula: &CnfFormula, limits: PruneLimits, skip_content_twins: bool) -> Option<Self> {
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

        Some(SatPruneCtx {
            solver,
            limits,
            pass_end: start,
            next_var,
            last_trigger_size: 0,
            consec_zero_passes: 0,
            spend_exempt_us: 0,
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

    /// Whether a pass fires on a diagram of `current_size` pairs: pruning
    /// has not stopped, the diagram is at least `threshold` and at most
    /// `max_pairs` pairs, and it has grown by `growth_factor` since the last
    /// pass. The caller runs [`SatPruneCtx::probe_tdd`] when it does.
    pub(crate) fn should_fire(&self, current_size: usize) -> bool {
        let limits = &self.limits;
        if self.stats.disabled_by.is_some() || current_size > limits.max_pairs {
            return false;
        }
        let min_size_for_next = (self.last_trigger_size as f32 * limits.growth_factor) as usize;
        current_size >= min_size_for_next.max(limits.threshold)
    }

    /// Count a pass that removed nothing. Once `max_consec_zero_passes` come
    /// in a row, pruning stops for the compile, unless it already has.
    pub(super) fn record_empty_pass(&mut self) {
        self.consec_zero_passes += 1;
        if self.consec_zero_passes >= self.limits.max_consec_zero_passes {
            self.stats.disabled_by.get_or_insert(PruneStop::EmptyPasses);
        }
    }

    /// The kill-rate gate: once `min_probes_for_rate_check` probes have run
    /// and fewer than `min_kill_rate` of them proved their node dead, pruning
    /// stops for the compile. Returns whether pruning has stopped.
    ///
    /// The walk calls it every `min_probes_for_rate_check` probes (the
    /// probe's `Probed` point, see `PassSolver`), and [`SatPruneCtx::probe_tdd`]
    /// once more after a walk.
    pub(super) fn maybe_disable_by_running_rate(&mut self) -> bool {
        if self.stats.disabled_by.is_some() {
            return true;
        }
        if self.stats.probes_attempted < self.limits.min_probes_for_rate_check {
            return false;
        }
        let rate = self.stats.probes_unsat as f32 / self.stats.probes_attempted as f32;
        if rate < self.limits.min_kill_rate {
            self.stats.disabled_by = Some(PruneStop::LowKillRate);
        }
        self.stats.disabled_by.is_some()
    }

    /// The tallies of every pass this context ran, and why pruning stopped,
    /// if it did.
    pub(crate) fn into_stats(self) -> SatPruneStats {
        self.stats
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
    /// every gated clause emitted so far is trivially satisfied, and the pass
    /// counts as one that removed nothing.
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
                self.ctx.record_empty_pass();
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
    /// counts, so the kill-rate gate reads every probe so far.
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
        let limits = &self.ctx.limits;
        let out_of_levels = matches!(at, EncodePoint::Level { complete, .. } if complete >= limits.max_encode_levels);
        if out_of_levels || self.start.elapsed().as_millis() as u64 >= limits.max_encode_ms {
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

    /// One `solve()` under `assumptions` and a conflict limit,
    /// `round_conflict_budget` for an enumeration round and
    /// `probe_conflict_budget` for a probe, bounded by the pass and counted in
    /// `sat_calls` and `probe_us` (a round also in `allsat_us`). The one solve
    /// site for the whole pruning context, so no probe can outrun the pass
    /// budget or go unbilled.
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
        let conflicts = if round { ctx.limits.round_conflict_budget } else { ctx.limits.probe_conflict_budget };
        ctx.solver.limit(c"conflicts", conflicts);
        for &literal in assumptions {
            ctx.solver.assume(literal);
        }
        let terminator = WallClockTerminator::new(Duration::ZERO);
        terminator.deadline_handle().set(ctx.pass_end + Duration::from_millis(ctx.limits.solve_overrun_ms));
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
    /// the walk once it is spent, and the walk at `max_probes_per_pass`
    /// probes or when the kill-rate gate fires. Kills recorded before a stop
    /// still go through the rewrite.
    fn poll(&mut self, at: ProbePoint, stats: &ProbeStats) -> ControlFlow<()> {
        let stop = match at {
            // One pass may spend the whole spend cap, and no more, before the
            // kill-rate gate gets to fire between passes.
            ProbePoint::Begin => {
                self.ctx.pass_end = Instant::now() + Duration::from_millis(self.ctx.limits.spend_cap_ms);
                false
            }
            // Shares the pass budget so the enumeration cannot starve the walk.
            ProbePoint::Round { .. } => self.ctx.pass_over(),
            ProbePoint::Solve { .. } => stats.probes >= self.ctx.limits.max_probes_per_pass || self.ctx.pass_over(),
            // The kill-rate gate, on its schedule (see
            // `maybe_disable_by_running_rate`).
            ProbePoint::Probed { .. } => {
                self.record(stats);
                self.ctx.stats.probes_attempted.is_multiple_of(self.ctx.limits.min_probes_for_rate_check)
                    && self.ctx.maybe_disable_by_running_rate()
            }
            _ => false,
        };
        if stop { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }
}

#[cfg(test)]
#[path = "tests/ctx.rs"]
mod tests;
