# Marginal TDD levels

In the counting modes the compiler needs a count, not a circuit. Once every
clause touching a vtree node's subtree has been conjoined, no later step can
change that node's level, so the compiler replaces the level with per-node
counts (weights, on a weighted compile) and frees its pairs. The library owns
the representation and what each operation does with it:

| Topic | Library item |
|---|---|
| Marginalization and the count it keeps | `Engine::marginalize`, and the library's [architecture reference](https://github.com/Tractables/tididi/blob/main/docs/architecture.md) |
| Downward closure, value-slot uniqueness | invariants 5 and 10 in the library's [invariant table](https://github.com/Tractables/tididi/blob/main/docs/architecture.md#invariants) |
| A clause reaching a marginal level | `OperationError::MarginalLevel` |
| Rotating next to marginal levels | `Engine::rotate_marginal_cluster` |
| The weighted arm | [weighted-marginal-levels.md](weighted-marginal-levels.md) |

This page covers what the compiler decides: which levels marginalize, when,
and how the peak is measured.

## Which modes marginalize

The model count, the weighted count and the projected count compile to a
counting `Output`: `Count`, `WeightedCount` or `ProjectedCount`. Circuit
output (`Mode::Compile`) compiles `Output::Function` and keeps every level
explicit. `Output::ProjectedFunction` forgets its projected variables on the
schedule below while keeping the remaining levels structural, so twin
contraction can merge identical subfunctions over the kept variables: `pmc`
uses `Output::ProjectedCount` in `projected_compile` and
`Output::ProjectedFunction` in `ShatterComponent::boundary_function` (before
marginalizing each component's private show leaves), and `pwmc` compiles
`Output::ProjectedFunction` so its weighted fold reads the whole diagram.

## The schedule

Every diagram of a compile is on a home, the vtree the compile was given,
and the compiler names what a step sums out by variable. A variable is done
at the highest-scoped step that touches it, or at its leaf's own step if that
is later; after it, no clause reads the variable. `VarSchedule`
(`src/compile/schedule.rs`) precomputes that step for each variable, and
which steps free anything at all. The library sums out a vtree node of the
diagram once every variable under it is done, and every node of a step's
diagram lies under the step, so no node is summed before its own step.

This is an early schedule: a node marginalizes as soon as its own content is
final, not when its parent's is. A projected count sums nothing out while a
projected variable is still to be forgotten, so the ∃-forget meets no
marginal level on its path.

## During the compile

Most marginalization happens inside a step's fold: `step_last_clauses`
finds the clause after which each variable the step completes is last
mentioned, so it is summed out at the merge of the batch that holds that
clause (`src/compile/batch.rs`). After each internal step, `run_marginalize_at`
(`src/compile/marginalize.rs`) marginalizes any scheduled level the fold left
and applies the projected ∃-forget. The marginal-clustering rotation pass runs
after that.

`peak_pairs` is the largest `tdd.pair_count()` observed at any step, taken
by `compile_component`'s per-step check after the step's fold (which has
already marginalized what it completed) and before the step-end pass. It is a
property of a compile run, not of the diagram it produced, so the compile
returns it beside the diagram (`Compiled::peak_pairs`); for a
multi-component compile it is the maximum over the per-component compiles,
which run one after another.

Because the root stays structural, `model_count` on a finished counting
diagram folds over the root level and reads its children's counts from their
marginal slots.

## Streaming over components

`compile_cnf` holds every component's diagram and conjoins them. For a count
alone, `model_count_streaming` (`src/compile/mod.rs`) compiles each
component, reads its count, drops the diagram and multiplies, so the peak is
the largest single component rather than the sum. The lift from a preprocessed
formula back to the input (`ArjunReduction::lift_pow2` and the driver's
own count lift) is folded once where a compile attempt's count is returned,
outside this loop. Its callers are the MC compile attempt
(`src/stage/mc/attempt.rs`) and `stage::count_streaming`
(`src/stage/mod.rs`), which the streaming MC under an explicit `--vtree` and
PWMC's symmetric-weight path share.
