//! SAT-based sound pruning of intermediate TDD nodes.
//!
//! Tseitin-encode the reachable partial TDD into a
//! long-lived `vitri::sat::CaDiCal` solver pre-loaded with the full original
//! CNF, then probe each reachable internal TDD node for liveness via
//! `assume(y_n) ∧ solve()` (the library's `Tdd::probe_nodes`, on this pass's
//! solver and stopping rules). A node is dead under Φ iff its function `f_n`
//! has no model that extends to a model of Φ. Once liveness is known we
//! propagate deadness bottom-up and *rewrite the TDD* by filtering dead pair
//! components out of every live node's pair list (the library's node filter,
//! finished by the compiler's full reduction to canonicalize). If the
//! rewrite leaves the root dead, the slot becomes `Tdd::zero`. (Sound: the
//! partial TDD is `∧ scoped-clauses-so-far`, all of which appear in Φ. If
//! `f_n ∧ Φ ≡ ⊥` then no model of Φ traverses `n`, so dropping every pair
//! that goes through `n` doesn't change what `Φ` evaluates to.)
//!
//! Per-pass clauses are gated by an activation literal so they can be
//! disabled cheaply after probing: emit each Tseitin clause with `¬a` as a
//! tail literal, then `add(-a); add(0)` at end-of-pass forces `¬a` true and
//! every gated clause is trivially satisfied. Original CNF clauses remain
//! active across passes.
//!
//! **Soundness corner.** `assume()` with `failed()` / `solve() → UNSAT`
//! reports unsatisfiability *under the assumptions plus active clauses*.
//! Active clauses include the original CNF and the just-added gated Tseitin
//! clauses (active because `a` is assumed). So UNSAT genuinely proves
//! `f_n ∧ Φ ≡ ⊥`. Unknown (conflict-limit hit) is treated as live — we
//! never kill on uncertain probes.
//!
//! The trigger threshold and the fixed limits are constants in `ctx.rs`; the
//! rest are fields of [`SatPruneCtx`], where `max_encode_ms` and `wall_ms`
//! are scaled from the run wall [`SatPrune`] carries (`--time-limit-ms`, or
//! `TIDIDI_TIME_LIMIT_MS` without the flag), with fixed values when the run
//! has none.

mod ctx;
mod probe;
mod stats;

pub(crate) use ctx::SatPruneCtx;
pub(crate) use probe::PruneOutcome;
pub(crate) use stats::SatPruneStats;

/// A run's SAT-prune pass: the wall its time knobs scale with, and the tallies
/// the run's compiles add to as each pass runs to its end. A compile handed
/// none runs no pass; the run hands none when `TIDIDI_SAT_PRUNE=off`
/// ([`crate::config::Config::sat_prune`]) turned the pass off.
#[derive(Default)]
pub(crate) struct SatPrune {
    wall_ms: Option<u64>,
    tally: std::cell::RefCell<Option<SatPruneStats>>,
}

impl SatPrune {
    /// The pass for a run whose wall is `wall_ms`, `None` when it has none.
    pub(crate) fn new(wall_ms: Option<u64>) -> Self {
        SatPrune { wall_ms, tally: std::cell::RefCell::default() }
    }

    /// The pass for one compile of `formula`, or `None` when no solver could
    /// be built.
    pub(crate) fn context(&self, formula: &vitri::cnf::CnfFormula, skip_content_twins: bool) -> Option<SatPruneCtx> {
        SatPruneCtx::new(formula, self.wall_ms, skip_content_twins)
    }

    /// Add what one pass did to the run's tallies.
    pub(crate) fn absorb(&self, stats: SatPruneStats) {
        self.tally.borrow_mut().get_or_insert_default().absorb(stats);
    }

    /// The run's tallies, `None` when no pass ran to its end.
    pub(crate) fn into_stats(self) -> Option<SatPruneStats> {
        self.tally.into_inner()
    }
}

#[cfg(test)]
mod tests;
