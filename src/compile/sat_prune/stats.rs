//! The tallies of a run's SAT pruning passes, and why pruning stopped, as
//! the `--stats` record reports them.

/// Why SAT pruning stopped for the rest of a compile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PruneStop {
    /// Passes in a row removed nothing.
    EmptyPasses,
    /// The probes proved too few of the nodes they checked dead.
    LowKillRate,
    /// Pruning spent its time cap without a rewrite that paid for it.
    TimeShare,
}

impl PruneStop {
    /// The reason as the `--stats` record's `sat_prune_disabled_by` spells it.
    pub(crate) fn key(self) -> &'static str {
        match self {
            PruneStop::EmptyPasses => "empty-passes",
            PruneStop::LowKillRate => "low-kill-rate",
            PruneStop::TimeShare => "time-share",
        }
    }
}

/// Declares [`SatPruneStats`]: each `u64` tally, with its doc comment, once.
/// From that one list come the struct, the sum [`SatPruneStats::absorb`]
/// takes and the `--stats` keys [`SatPruneStats::tallies`] yields, so a new
/// counter cannot be left out of either.
macro_rules! sat_prune_stats {
    ($($(#[$doc:meta])* $field:ident,)*) => {
        /// What one run's SAT pruning passes did, as the `--stats` record reports it.
        #[derive(Default, Clone, Debug)]
        pub(crate) struct SatPruneStats {
            $($(#[$doc])* pub(crate) $field: u64,)*
            /// Why pruning stopped before the compile ended; `None` while it runs.
            pub(crate) disabled_by: Option<PruneStop>,
        }

        impl SatPruneStats {
            /// Add `other`'s counters into these; `disabled_by` takes `other`'s
            /// reason when it has one.
            pub(crate) fn absorb(&mut self, other: SatPruneStats) {
                $(self.$field += other.$field;)*
                self.disabled_by = other.disabled_by.or(self.disabled_by);
            }

            /// Each tally under its `--stats` key, `sat_prune_` and the field's name.
            pub(crate) fn tallies(&self) -> impl Iterator<Item = (&'static str, u64)> {
                [$((concat!("sat_prune_", stringify!($field)), self.$field),)*].into_iter()
            }
        }
    };
}

sat_prune_stats! {
    /// Number of times the size-threshold trigger fired.
    passes_triggered,
    /// Sum of the diagram's [`Tdd::pair_count`] at the moment of each trigger.
    /// Divide by `passes_triggered` for the average subcircuit size at trigger.
    pairs_inspected,
    /// Number of internal TDD nodes probed across all passes.
    probes_attempted,
    /// Number of probes that returned UNSAT (i.e. proved-dead nodes).
    probes_unsat,
    /// Number of probes that returned unknown (budget hit; conservatively
    /// treated as live).
    probes_unknown,
    /// Number of nodes that turned out dead either by direct SAT-UNSAT or
    /// by transitive propagation (all input pairs have a dead component).
    nodes_pruned,
    /// Number of times the bottom-up propagation reached the TDD root and
    /// the root was dead. (Each such pass returns `DeadRoot`.)
    passes_dead_root,
    /// Number of rewrites that finished, including those that collapsed to
    /// ⊥ and returned `DeadRoot`.
    passes_rewritten,
    /// Total input pairs filtered out across all rewrites (sum of pairs
    /// dropped from every node's pair list).
    pairs_filtered,
    /// Wall time spent in the rewrite (the node filter and its full
    /// reduction), accumulated in microseconds.
    rewrite_us,
    /// Total `solver.solve()` calls issued: per-node probes plus AllSAT
    /// rounds.
    sat_calls,
    /// Wall time to construct the solver and load the CNF, in microseconds.
    solver_setup_us,
    /// Wall time spent inside `solver.solve()` calls, in microseconds.
    probe_us,
    /// Wall time spent Tseitin-encoding partial TDDs (separate from probe),
    /// in microseconds.
    encode_us,
    /// Passes aborted mid-encode because `max_encode_ms` was exceeded.
    /// Activation literal `a` is retired before bail-out so the
    /// partially-emitted clauses are inert in subsequent passes.
    passes_aborted_encode_budget,
    /// Passes that hit the encode budget but kept the
    /// fully-encoded bottom-up prefix and probed it instead of
    /// discarding the pass.
    passes_encode_prefix,
    /// Allocated y-slots zeroed because they lie at or above the
    /// encode-prefix break point; the prelude and the walk then treat them as
    /// unreachable.
    prefix_nodes_skipped,
    /// Nodes killed via the MC=0 short-circuit (counted in
    /// `nodes_pruned` but reported separately for attribution).
    zero_count_kills,
    /// Total wall time spent computing per-node model counts, summed across
    /// all probe passes, in microseconds.
    count_order_us,
    /// Number of internal TDD nodes that were alive entering the rewrite
    /// but had their pair list collapse to empty during it (every pair
    /// referenced a node killed earlier in the pass): the node filter's
    /// emptied nodes. These kills are not in `nodes_pruned`, so the kills
    /// split into direct ones (`probes_unsat + zero_count_kills +
    /// allsat_kills`), propagation (`nodes_pruned` less the direct ones) and
    /// this field.
    rewrite_propagated_kills,
    /// Model-witness aliveness: number of internal TDD nodes that
    /// would have been probed but were skipped because a prior SAT probe's
    /// satisfying model already certified them alive (model assigns the
    /// node's y-literal to true under the same activation `a`). Each
    /// witness saves one SAT probe.
    nodes_skipped_witness,
    /// Model-witness aliveness: number of `solver.val()` queries
    /// issued during witness harvesting, from AllSAT rounds and SAT probes
    /// alike: one per reachable internal node the model is asked about.
    witness_val_queries,
    /// AllSAT prelude: number of unconditioned global solves issued
    /// (assume={a, a_block}, no y_n target). Subset of `sat_calls`.
    allsat_global_attempted,
    /// How many of those returned SAT (harvested a model).
    allsat_global_sat,
    /// How many of those returned UNSAT (terminating enumeration,
    /// licensing the "every unwitnessed node is dead" conclusion).
    allsat_global_unsat,
    /// How many of those returned unknown (budget hit; no new
    /// witnesses, falls back to individual probes).
    allsat_global_unknown,
    /// Nodes first witnessed alive by an AllSAT round (a node already
    /// witnessed by an earlier round is not counted again).
    allsat_witnesses_harvested,
    /// Nodes proved dead via AllSAT-UNSAT termination (every reachable
    /// internal node that wasn't
    /// witnessed in any of the K SAT models is dead by exhaustion).
    /// Subset of `nodes_pruned`. Disjoint from `probes_unsat`
    /// and `zero_count_kills`.
    allsat_kills,
    /// Wall time spent in the AllSAT prelude's solves, in microseconds.
    /// Subset of `probe_us`.
    allsat_us,
    /// Number of level walks in which at least one node had a nonzero
    /// dead-child score, so the probe order put it first.
    dead_child_reorders,
}

impl SatPruneStats {
    /// Microseconds spent solving, encoding, rewriting and counting: the
    /// spend `PruneLimits::spend_cap_ms` caps.
    pub(super) fn spend_us(&self) -> u64 {
        self.probe_us + self.encode_us + self.rewrite_us + self.count_order_us
    }
}
