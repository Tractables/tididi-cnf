# SAT-based TDD pruning

Mid-compile pruning of intermediate diagram nodes that no model of the original
CNF Φ passes through. Once a partial diagram passes the size trigger, reachable
internal nodes are probed for `f_n ∧ Φ` satisfiability against a long-lived
CaDiCaL instance that holds Φ, up to `MAX_PROBES_PER_PASS` per pass. An UNSAT
node's pairs are dropped and the diagram is re-minimized. CNF preprocessing
prunes the input; this prunes the partial output, catching nodes that look
live in isolation but that no model of Φ passes through.

The pass runs on every compile without a weight store, so in every mode but
WMC, after each vtree step's clauses are folded in (`src/compile/step.rs`). It
stands down for the rest of a compile once it stops paying;
`SatPruneStats::disabled_by` lists the reasons. `TIDIDI_SAT_PRUNE=off` turns
it off for a whole run, a diagnostic off-switch for paired on/off
measurements ([`env.md`](env.md#tididi_sat_prune)): the command line
reads it into `Config::sat_prune`, the driver then hands the run's compiles
no `SatPrune` pass, and `compile_component` builds no solver.

Where each part is documented:

- the mechanism and its soundness: the `src/compile/sat_prune/` module doc;
- when a pass fires: `SatPruneCtx::should_fire`;
- the encoding, marginal levels included: `PassSolver::encode` and the
  library's `Tdd::encode_cnf`;
- the probe: the library's `Tdd::probe_nodes`, on the solver and stopping
  rules of `PassSolver` (`src/compile/sat_prune/ctx.rs`);
- the fixed limits, the size threshold `SAT_PRUNE_THRESHOLD` among them: the
  constants in `src/compile/sat_prune/ctx.rs`.

The `sat_prune_*` fields of the `--stats` record report per-run pass counts,
timings and the disable reason; see `docs/stats.md`.
