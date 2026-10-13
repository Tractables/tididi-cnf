//! The compile's fixed parameters: batch size and width cap, the reduction
//! cadence and the clustering bound. The SAT-prune pass keeps its own, in
//! `sat_prune`.

/// Clauses per batch.
pub(crate) const BATCH_SIZE: usize = 32;

/// After a full minimize, the size threshold is this multiple of the
/// diagram's node count.
pub(crate) const MIN_SIZE_MULT: f64 = 1.1;

/// Width cap for a single batch TDD once the accumulator is wider than
/// [`UNCAPPED_ACC_WIDTH`]: the batch stops growing when its `max_width()`
/// exceeds this (`batch_width_cap`).
pub(crate) const BATCH_WIDTH_LIMIT: usize = 256;

/// The accumulator width up to which a batch grows with no width cap.
pub(crate) const UNCAPPED_ACC_WIDTH: usize = 64;

/// Prune growth trigger multiplier. Prune fires when total nodes exceed
/// `last_prune_nodes * PRUNE_GROWTH_TRIGGER`.
pub(crate) const PRUNE_GROWTH_TRIGGER: f64 = 1.05;

/// Bound multiplier for the mid-compile marginal-clustering rotation pass,
/// which runs on every marginalizing compile: it shrinks the in-flight diagram
/// at neutral cost, and bails read-only when a step produced no clusterable
/// marginal levels. The multiplier caps the restructure pair-count blow-up the
/// clustering rotation is allowed before the collapse removes it; the per-level
/// cost cap is the library's, inside `Engine::rotate_marginal_cluster`. Cost is kept local inside the pass
/// (per-rotation two-level size delta, attempt-once per pivot, per-level cap) —
/// no whole-diagram gate.
pub(crate) const MARGINAL_CLUSTER_BOUND_FACTOR: usize = 32;
