# Marginal TDD levels

In the counting modes the compiler needs a count, not a circuit. Once every
clause touching a vtree node's subtree has been conjoined, no later step can
change that node's level, so the compiler replaces the level with per-node
counts (weights, on a weighted compile) and frees its pairs. The library owns
the representation and what each operation does with it:

| Topic | Library item |
|---|---|
| Marginalization and the count it keeps | `Engine::marginalize_levels`, and the library's [architecture reference](https://github.com/Tractables/tididi/blob/main/docs/architecture.md) |
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
explicit. The projected weighted count compiles `Output::ProjectedFunction`,
which forgets its projected variables on the schedule below but sums no
level out, so its weighted fold reads the whole diagram.

## The schedule

For each vtree node d, *completion(d)* is the last compilation step that can
modify d's level: the highest-scoped step that touches any variable in d's
subtree, or d's own step if that is later, since d's children are joined
there. `marginalize_schedule` (`src/compile/schedule.rs`) precomputes it:

1. For each variable, find the highest-scoped clause mentioning it.
2. Bottom-up, `completion_pos[t] = max(completion_pos[left], completion_pos[right], pos(t))`
   for internal nodes; a leaf takes the later of its variable's last-scope
   position and its own.
3. Group the nodes by the step that frees them, in bottom-up order. The root
   is never scheduled.

This is an early schedule: d marginalizes as soon as its own content is final,
not when its parent's is.

## During the compile

Most marginalization happens inside a step's fold: `step_completions`
refines the step's group down to the clause after which each subtree is last
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

`compile_cnf` holds every component's diagram and grafts them. For a count
alone, `model_count_streaming` (`src/compile/mod.rs`) compiles each
component, reads its count, drops the diagram and multiplies, so the peak is
the largest single component rather than the sum. The lift from a preprocessed
formula back to the input (`ArjunReduction::lift_pow2` and the driver's
own count lift) is folded once where a compile attempt's count is returned,
outside this loop. Its callers are the MC compile attempt
(`src/stage/mc/attempt.rs`) and `stage::count_streaming`
(`src/stage/mod.rs`), which the streaming MC under an explicit `--vtree` and
PWMC's symmetric-weight path share.
