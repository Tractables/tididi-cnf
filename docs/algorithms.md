# Algorithms

How the compiler turns a CNF into a diagram. The diagram operations it calls —
clause conjunction, `Engine::and` and reduction — belong to the library, and
[tididi's architecture reference](https://github.com/Tractables/tididi/blob/main/docs/architecture.md)
describes them.

## Vtree-guided bottom-up compilation

Instead of conjoining all clauses at the root (Algorithm 1 from the paper),
each clause is conjoined at the lowest vtree node covering all its variables.
That keeps intermediate TDDs much smaller.

```
1. For each clause, compute its scope = LCA of its variables' vtree leaves
2. Group clauses by scope, and order each group
3. Bottom-up traversal of the vtree:
   - At each internal node t, conjoin child TDDs (from left/right subtrees)
   - Then batch-accumulate clauses scoped to t (see below), or fold them
     once per assignment of a few-variable child (the cofactor step)
4. Return the TDD at the root
```

**Scope**: `clause_scope` (`src/compile/scope.rs`) folds `Vtree::lca` over the
clause's leaves.

**Clause order within a node**: the order of a node's clauses cannot change
what the node conjoins, only the intermediate diagrams of its fold.
`order_group` (`src/compile/scope.rs`) orders each group of more than two
clauses by one of two orders:

- The *proximity tour* starts at the clause with the most variables and
  repeatedly takes the unplaced clause sharing the most variables with the
  last one placed.
- The *completion order* is a greedy over *open* variables: a variable opens
  when a placed clause first mentions it and finishes when its last clause in
  the group is placed and the node is its last scope (the highest scope of a
  clause mentioning it, `last_scope_positions`); one whose last scope is
  higher stays open. The greedy places next the clause that leaves the fewest
  open, ties to the one finishing more, then to the earlier clause. *Pendant*
  clauses, none of whose variables occurs in another clause of the group and
  all of whose variables finish at the node, go last, in group order: the
  open count scores them free, but each couples the node's two sides and
  multiplies the accumulator until its variables are summed out, so placed
  early it enlarges every later step of the fold.

Each order is scored by its open profile, the largest and the summed open
count after each clause. Where some variable finishes at the node, the
completion order replaces the tour only when it is clearly smaller on both
(`COMPLETION_ORDER_MAX_RATIO`, `COMPLETION_ORDER_SUM_RATIO`). The profile is
only a proxy for the diagrams' size, and the margin keeps the tour on
near-ties, where the completion order can score a little smaller and still
build far larger diagrams: bipartite stars of two-literal clauses, where it
opens one side's hubs before it finishes any. Where no variable finishes,
both orders end with every variable open, so the maxima tie and only the
order of touching differs; a smaller sum suffices there. Groups over
`COMPLETION_ORDER_MAX_CLAUSES` keep the tour without computing the
completion order, whose greedy is quadratic in the group.

**Why this is faster**: A clause scoped to a sub-vtree only affects widths
within that subtree, leaving ancestor levels at width 1. The naive
root-scoped approach creates large intermediate TDDs where every clause
inflates every level.

### Batched clause merging

Two extremes for handling local clauses at a vtree node:
- **One-by-one into recursive**: each clause conjoined directly with the
  (large) recursive TDD. Safe, but every clause triggers an expensive
  `Engine::and`.
- **Batch-first**: build a TDD from all local clauses, then conjoin with the
  recursive TDD. Can blow up without the recursive constraints holding it
  down.

**Batch strategy**: build a small TDD from a batch of clauses (clause ×
clause operations are cheap), then merge the batch result into the
accumulator with `merge_batch_into_acc`. Where a batch closes is decided in
`build_one_batch`:

- **Floor.** A batch folds `BATCH_SIZE` clauses unless it becomes UNSAT, the
  node's clauses run out, or — once the accumulator is wider than
  `UNCAPPED_ACC_WIDTH` — its width passes `BATCH_WIDTH_LIMIT`
  (`batch_width_cap`). A batch the width cap stops never extends: one that
  turns wide before the floor is unlike the absorbed batches that earned the
  extension, and merging it wide into a wide accumulator multiplies the
  product.
- **Large floor.** A node whose fold starts from an accumulator of more than
  `BATCH_LARGE_ENTRY_PAIRS` pairs uses `BATCH_LARGE_SIZE` and
  `BATCH_LARGE_WIDTH_LIMIT` as its floor and below-floor cap instead
  (`batch_floor`, fixed for the fold). Each merge into such an accumulator
  costs in proportion to its pairs whether or not the clauses multiply it,
  and the earned extension below only lengthens batches after absorbing
  merges, so a large node whose merges expand would otherwise pay a full
  accumulator merge every `BATCH_SIZE` clauses. The decision reads the
  library's pair total, which caches the result for later batch decisions
  and counts implicit levels without scanning their nodes.
- **Extension.** A merge into a wide accumulator costs time in proportion to
  the accumulator's pairs, nearly independent of the batch, while conjoining
  one more clause costs time in proportion to the batch's pairs. Past the
  floor, a batch keeps folding while the next clause's build cost stays below
  the running per-clause average of its build so far plus its merge
  (`BATCH_MERGE_COST_RATIO` puts an accumulator pair in units of a
  batch-pair conjoin). An extended batch also closes when its width passes
  `BATCH_WIDTH_GUARD` (once the accumulator is wider than
  `UNCAPPED_ACC_WIDTH`) and after a clause that is a marginalize target's
  last mention, so extension never delays a marginalization.
- **Earned extension.** A batch may extend only after the previous merge at
  the same node was absorbing — its unreduced product held at most
  `BATCH_ABSORB_FACTOR` times the accumulator's pairs — and then to at most
  `BATCH_GROWTH` times the clauses of the batch just merged
  (`next_batch_limit`). The first merge that multiplies the accumulator
  sends the node back to floor-sized batches: a node's clauses can turn from
  absorbing to expanding, and one long batch merged across that turn
  multiplies the unreduced product far beyond what the same clauses cost in
  floor-sized merges.

All the constants are in `src/compile/batch.rs`.

**Reduce within the batch**: after each clause conjunction inside the batch,
inner-node twins are contracted (`ReductionPlan::Contract`), so each
successive conjunction works on a smaller input.

**Early exit**: if the batch conjunction becomes UNSAT, the remaining clauses
in the batch are skipped; if the accumulator conjunction becomes UNSAT, the
compile returns early.

### Cofactor step

At an internal node whose smaller child spans at most `COFACTOR_MAX_VARS`
variables `S`, and which has at least `COFACTOR_MIN_CLAUSES` clauses, the
batched fold above is replaced by a fold per assignment of `S`
(`cofactor_step`, `src/compile/cofactor.rs`). Every clause scoped at the node
has literals on both sides, so each merge of the normal fold carries the small
side's structure across the whole of the other child's diagram `R`. Instead,
for each full assignment `a` of `S` that the small child's diagram `L` allows,
the step folds the node's clauses that `a` leaves unsatisfied, with their `S`
literals removed, into a copy of `R` with the same batched fold, shortest
clause first. It drops the branches that come back ⊥ and joins the rest as the
disjunction of `cube(a) ∧ R_a`.

- **When.** Only if neither child's diagram has a marginal level
  (`Tdd::has_marginal_level`). A small child with no diagram yet is the
  constant true, and every assignment is a branch.
- **Why it is exact.** With no marginal level, both children are the exact
  Boolean functions of their variables, and `L` depends on `S` alone. A full
  assignment `a` makes `L` a constant and leaves of the node's clauses exactly
  the unsatisfied ones less their `S` literals, so the disjunction over the
  allowed `a` is `L ∧ R ∧ C_t` itself. Weighted compiles take the step too:
  weight values live only at marginal levels, so the branches carry none, and
  the weights enter when the node's scheduled levels are marginalized
  afterwards.
- **Marginalization.** The branch folds marginalize nothing. Once the
  branches are joined, the step marginalizes the levels the normal fold would
  have summed out inside its merges (`build_marginalize_targets`). Then
  `run_marginalize_at` forgets and marginalizes the node's schedule, as after
  any step, and the SAT pruning trigger sees the result.
- **Cost guard.** Each branch costs at least one merge into a copy of `R` plus
  a disjunction in the join. The normal fold merges about once per floor's
  worth of clauses (`batch_floor` at `R`). So the step is taken only when
  `ceil(clauses / floor)` exceeds the number of branches, a test made before
  any fold.

The shortest-first order matters: the shortest clauses constrain a branch
most, so folding them first keeps small the diagram the longer ones meet.

### Independent component compilation

When a CNF decomposes into clause groups sharing no variables, each component
compiles against its own small vtree and the results are joined — avoiding
one identity level per component variable across the full vtree.

`CnfFormula::detect_components` partitions the clauses. Each component
compiles via `compile_component` against its *own* `Arc<Vtree>`, and
`Tdd::graft_over` joins the results structurally, with no cross-component
conjunction: the grafted vtree chains the component trees together, and each
variable in no clause joins as a free leaf that doubles the model count. The
compiler then runs a full reduction on the grafted diagram. A weighted
compile passes the graft an empty store, and each component's values move
into it.

Components must use independent vtrees: if they shared the full vtree,
rotations during one component's compilation would corrupt sibling TDDs
through stale identity levels and mismatched vtree `Arc`s.

### Marginal TDD levels and peak compile size

[Marginal TDD levels](marginal-tdd-levels.md) has the design: marginal
sub-vtree levels replaced with per-node model counts in `mc` mode, and the
`peak_pairs` measurement.

## Fallback stages

A compile can give up on a vtree that another tree, or another form of the
formula, would compile. These stages take over; the record's `answered_by`
names the one that answered ([`stats.md`](stats.md)).

- **The `compile` tree.** Under `compile`, a large, clause-dense formula that
  splits into independent parts, none of which dominates the split, gets
  vitri's force-directed `force` tree on every part instead of the
  portfolio's pick
  ([vitri's vtree guide](https://github.com/Tractables/vitri/blob/main/docs/vtrees.md)
  gives the rule). On some formulas the diagram outgrows the memory limit on
  the portfolio's pick while another tree compiles in a small fraction of it.
- **Bisection retry** (`bisect-retry`). When a `compile` runs out of memory
  with time left, it compiles once more on vitri's hypergraph bisection,
  `hypergraph-bisect:imbalance=0.40`, unless the portfolio's pick already was
  a `hypergraph-bisect` tree. The retry builds its tree in the time left, and
  one that cannot finish the tree gives up. Nothing follows the retry.
- **Stall recompile** (`stall-recompile`). Under `mc`, a compile of the
  reduced formula that stalls is given up early. The compile works up the
  vtree one node at a time, and with a time limit each node's step may run
  for a sixth of it, as long as at least 50 steps, that one included, are
  still to come in its part of the formula. A step that runs longer ends the
  compile, and the same reduced formula is compiled again, to the end of the
  run, on a vtree from vitri's portfolio restricted to `goatd-primal`, a
  construction the portfolio otherwise leaves out; the record's
  `vtree_candidate` names the tree. A recompile that gives up leaves the
  formula to the stages that follow any failed compile. Without a time limit,
  no step has a limit of its own. `wmc` has no such step limit.
- **Monotone reduction** (`monotone-reduction`). Under `mc`, the run asks
  vitri to skip Arjun on a monotone formula, one in which every variable
  occurs in one polarity only: its compile usually answers sooner without the
  reduction. When that compile gives up with more than five and a half
  minutes left, Arjun reduces the formula after all and the reduction, when
  smaller, is compiled, leaving a quarter of the time left, and at least five
  minutes, to the steps after it.
- **Later stages.** Behind a failed compile, `mc` tries the no-SBVA retry,
  support enumeration and the unreduced retry, then splits the formula on a
  variable; [the support-enumeration stage](support-enum-stage.md) says how
  they are ordered.
