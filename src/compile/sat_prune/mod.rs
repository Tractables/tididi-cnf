//! SAT pruning: removing the intermediate TDD nodes no model of the formula
//! passes through.
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
//! [`PruneLimits`] holds every limit a pass runs under. Several of them
//! scale with the run's time limit, `--time-limit-ms` or, without the flag,
//! `TIDIDI_TIME_LIMIT_MS`; [`PruneLimits::for_wall`] says which, and
//! [`SatPrune`] carries them.
//!
//! **Spend caps.** A context's unproductive spend, its spend since its last
//! productive rewrite, is capped at `PruneLimits::spend_cap_ms`. A compile
//! builds one context per component, and its contexts also share a cap,
//! `PruneLimits::compile_spend_cap_ms`: each context starts from the
//! unproductive spend of the ones before it, the compile hands that total on
//! as a context closes, and no pass gets more of a budget than the compile's
//! cap has left.

mod ctx;
mod probe;
mod stats;

use std::cell::RefCell;

use vitri::cnf::CnfFormula;

pub(crate) use ctx::{PruneLimits, SatPruneCtx};
pub(crate) use probe::PruneOutcome;
pub(crate) use stats::{PruneStop, SatPruneStats};

/// A run's SAT pruning: the limits its passes run under, and the tallies the
/// run's compiles add to as each runs to its end. A compile handed none runs
/// no pass; the run hands none when `TIDIDI_SAT_PRUNE=off`
/// ([`crate::config::Config::sat_prune`]) turned pruning off.
pub(crate) struct SatPrune {
    limits: PruneLimits,
    tally: RefCell<Option<SatPruneStats>>,
}

impl Default for SatPrune {
    /// Pruning for a run with no time limit.
    fn default() -> Self {
        SatPrune::new(None)
    }
}

impl SatPrune {
    /// Pruning for a run whose time limit is `wall_ms`, `None` when it has
    /// none.
    pub(crate) fn new(wall_ms: Option<u64>) -> Self {
        SatPrune { limits: PruneLimits::for_wall(wall_ms), tally: RefCell::default() }
    }

    /// The pruning context for one component of a compile, `formula`, or
    /// `None` when no solver could be built; `compile_spent_us` is the
    /// unproductive spend of the compile's earlier contexts.
    pub(crate) fn context(
        &self,
        formula: &CnfFormula,
        skip_content_twins: bool,
        compile_spent_us: u64,
    ) -> Option<SatPruneCtx> {
        SatPruneCtx::new(formula, self.limits, skip_content_twins, compile_spent_us)
    }

    /// Add the tallies of one component's pruning context, every pass it
    /// ran, to the run's.
    pub(crate) fn absorb(&self, stats: SatPruneStats) {
        self.tally.borrow_mut().get_or_insert_default().absorb(stats);
    }

    /// The run's tallies, `None` when no component's compile ran to its end.
    pub(crate) fn into_stats(self) -> Option<SatPruneStats> {
        self.tally.into_inner()
    }
}

