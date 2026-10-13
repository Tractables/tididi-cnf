# Algorithms

How the compiler turns a CNF into a diagram. The diagram operations it calls —
clause conjunction, `Engine::and` and reduction — belong to the library, and
[tididi's architecture reference](https://github.com/Tractables/tididi/blob/main/docs/architecture.md)
describes them.

## Vtree-guided bottom-up compilation

Instead of conjoining all clauses at the root (Algorithm 1 from the paper), each clause is conjoined at the lowest vtree node covering all its variables. That keeps intermediate TDDs much smaller.

```
1. For each clause, compute its scope = LCA of its variables' vtree leaves
2. Group clauses by scope
3. Bottom-up traversal of the vtree:
   - At each internal node t, conjoin child TDDs (from left/right subtrees)
   - Then batch-accumulate clauses scoped to t (see below)
4. Return the TDD at the root
```

**Scope**: `clause_scope` (`src/compile/scope.rs`) folds `Vtree::lca` over the clause's leaves.

**Why this is faster**: A clause scoped to a sub-vtree only affects widths within that subtree, leaving ancestor levels at width 1. The naive root-scoped approach creates large intermediate TDDs where every clause inflates every level.

### Fixed-batch clause merging

Two extremes for handling local clauses at a vtree node:
- **One-by-one into recursive**: each clause conjoined directly with the (large) recursive TDD. Safe, but every clause triggers an expensive `Engine::and`.
- **Batch-first**: build a TDD from all local clauses, then conjoin with the recursive TDD. Can blow up without the recursive constraints holding it down.

**Fixed-batch strategy**: build a small TDD from a batch of clauses (clause × clause operations are cheap), then merge the batch result into the accumulator with `merge_batch_into_acc`. A batch closes at `BATCH_SIZE` clauses, when it becomes UNSAT, or — once the accumulator is wider than 64 — when its width passes `BATCH_WIDTH_LIMIT` (`batch_width_cap`). Both constants are in `src/compile/config.rs`.

**Reduce within the batch**: after each clause conjunction inside the batch, inner-node twins are contracted (`ReductionPlan::Contract`), so each successive conjunction works on a smaller input.

**Early exit**: if the batch conjunction becomes UNSAT, the remaining clauses in the batch are skipped; if the accumulator conjunction becomes UNSAT, the compile returns early.

### Independent component compilation

When a CNF decomposes into clause groups sharing no variables, each component compiles against its own small vtree and the results are joined — avoiding one identity level per component variable across the full vtree.

`CnfFormula::detect_components` partitions the clauses. Each component compiles via `compile_component` against its *own* `Arc<Vtree>`, and `Tdd::graft_over` joins the results structurally, with no cross-component conjunction: the grafted vtree chains the component trees together, and each variable in no clause joins as a free leaf that doubles the model count. The compiler then runs a full reduction on the grafted diagram. A weighted compile passes the graft an empty store, and each component's values move into it.

Components must use independent vtrees: if they shared the full vtree, rotations during one component's compilation would corrupt sibling TDDs through stale identity levels and mismatched vtree `Arc`s.

### Marginal TDD levels and peak compile size

`docs/marginal-tdd-levels.md` has the design: marginal sub-vtree levels replaced with per-node model counts in `mc` mode, and the `peak_pairs` measurement.

The library answers every query on the compiled diagram: model counting,
weighted evaluation, size and Graphviz output. Its
[API overview](https://github.com/Tractables/tididi/blob/main/docs/api-guide.md)
names the item for each, and `docs/output.md` shows how to load a circuit that
`--save-tdd` wrote.
