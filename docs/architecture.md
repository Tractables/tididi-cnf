# Architecture

## Context

The paper "A canonical generalization of OBDD" (by Capelli, Choi, Mengel, Muñoz, Van den Broeck) introduces Tree Decision Diagrams (TDD), a knowledge compilation language that generalizes OBDDs using vtrees instead of linear variable orders. TDDs support the same tractable operations as OBDDs (apply, negation, conditioning) plus canonical minimality via twin contraction. CNF formulas of bounded treewidth compile to TDDs of FPT size via bottom-up compilation. That size is a worst-case upper bound; compiled diagrams are usually far smaller.

## The run

`driver::solve` is the run, and the binary and the library's `count` both
make it: `cli::run_cli`, the only entry the binary calls, parses argv, calls
`solve` and prints what it answered, and `count` calls `solve` with the
caller's mode and limits. The C and Python bindings under `bindings/`, crates
of their own outside the workspace, call `count` and convert its arguments and
answer; they hold no logic of their own. One run goes:

1. **Arguments to `Config`.** `cli::parse_args` turns argv into a `Cli` — the
   `--mode` flag, the `Limits`, the `Settings` and where the answer goes.
   `cli::run_cli` hands the mode, limits and settings to `driver::solve`;
   `count` passes its own and default settings. `solve` lowers the address-space limit to the
   run's memory limit and puts it back when the run returns; inside it,
   `driver::parse_input` builds the run's configuration record with
   `Config::new` and starts the clock.
2. **DIMACS to formula.** `parse_dimacs` reads the instance through vitri's
   reader and hands back vitri's `CnfFormula` plus a `CnfMeta` carrying the
   `c t` mode header, the `c p show` projection set and the `c p weight` table.
   A library caller holding clauses as DIMACS integers builds the same pair
   with `formula_from_clauses`.
3. **Dispatch.** `Mode::resolve` merges `--mode` with the `c t` / `c p show`
   header into the mode — plain compile, MC, WMC, PMC or PWMC — refusing a
   `--mode` the header contradicts and `--mode mc` or `--mode pmc` beside
   `c p weight` lines. `Settings::refuse_inert` then refuses every setting
   that mode would ignore, all of those checks in one place, and `driver`
   dispatches on the mode; MC takes the cascade unless `--vtree` names a
   spec.
4. **Preprocessing and vtree, per route.** The compile, MC cascade and WMC
   routes get the reduced formula and its vtree from one vitri call
   (`frontend`). MC under an explicit `--vtree` simplifies once and
   builds a vtree per component through `vtree`. PMC and PWMC run their
   own projection preprocessing. `--preprocess` sets vitri's simplify chain
   ([vitri's preprocessing guide](https://github.com/Tractables/vitri/blob/main/docs/preprocessing.md))
   on the other routes, and PMC and PWMC refuse it: their projection chain
   always simplifies;
   Arjun runs on the MC cascade and inside the WMC, PMC and PWMC stages.
5. **Answer.** Each mode's runner returns what it answered as a typed value,
   and the driver records it once through `output` — the verdict, the count
   and the run's record — and `solve` returns them as an `Answer`. For the binary, `output::finish` prints the
   answer and, under `--stats`, writes the record. `docs/stats.md` is the
   field table.

## The module graph

`driver/` runs and dispatches; `stage/` holds the stage bodies it dispatches to;
everything below is called, and nothing below calls back up. Below the stages,
`frontend` builds on `vtree` and `problem`, and `vtree` on `convert`.

| module | what it holds |
|---|---|
| `src/run.rs` | The library's one call, `count`, with its `Input` and `Answer`. |
| `src/cli.rs` | The command line: argument parsing, the usage text, and `run_cli`, which solves what argv asks and prints the answer. |
| `src/driver/` | `solve`, the phase spine, the mode dispatch, and the plain compile route. |
| `src/stage/mc/` | The model count. The cascade: Arjun reduces the formula, the primary compile runs on the reduction, then the fallbacks, support enumeration among them, and Shannon split when they fail. The streaming compile under an explicit `--vtree`, with Shannon split behind it. One recovery orders both. |
| `src/stage/` | `Run`, what every stage reads — the engine, the configuration, the run's wall and the SAT-prune pass its compiles share — and `run_isolated`, which a leaf compile that may panic runs under; `count_streaming`, the component-at-a-time count the model count and the weighted projected count share; the WMC, PMC and PWMC runners the driver dispatches to, each returning its answer for the driver to record; the stages the two projected counts share, run in order until one answers, from a list each picks once by whether the problem has a show set; and PMC's show-variable conditioning and its count over a separator. |
| `src/enumeration.rs` | Counting a show set's feasible assignments with a SAT solver: the small-show sweep, projected AllSAT, and the estimates that gate them. |
| `src/compile/` | `compile_cnf`, what it takes and what it returns; the bottom-up pass beneath it; and the counts the stages build on it. See below. |
| `src/problem.rs` | The counting problem every runner works on: a formula, its show set, its weights and the domain of its answer, and the weighted count that carries whether the formula has a model. |
| `src/vtree.rs` | Vtree construction from a spec, whole or one tree per independent component: the public calls and their options, the vitri request and selection context every construction makes, the conversion of the trees vitri returns, and the descriptors of a formula's independent components. |
| `src/frontend/` | The run's calls into vitri's preprocessing. `mod.rs` runs them, preprocessing paired with a vtree or alone; `request.rs` builds every vitri request from the run's configuration and reads vitri's environment settings when a run starts; `record.rs` reads what a call returns: the problem it reduced to, the Arjun reduction, and a compile's map back to the original variables; `expand.rs` puts a compiled diagram back into those variables. |
| `src/convert.rs` | The conversions between vitri's types and the diagram's — a variable, a clause literal, a vtree — and a weight table as the diagram's weighted-counting algebra. |
| `src/formula.rs` | `parse_dimacs`, and the occurrence count and one-literal conditioning the case splits share. |
| `src/output.rs` | Every stdout line the run emits, and the record `--stats` writes. |
| `src/error.rs` | `Error`, the one error type the binary reports, whose variant fixes the exit status. |
| `src/config.rs` | The mode, the limits, the command line's other settings and the one check that refuses a setting the mode would ignore, and the run's configuration record. |
| `src/budget.rs`, `src/mem.rs` | The wall-slicing policy and the allocator hooks. |
| `src/mem_wasm.rs` | `mem` on WebAssembly: the same functions, inert, with no jemalloc and no address-space limit; it refuses a memory limit. |
| `src/guide.rs` | The guide in the crate documentation: the worked examples, `docs/examples/`, and this page. |

### Inside the compiler

`compile_cnf` takes a formula, a vtree and `CompileOptions`: a
`ComponentPlan` (find the independent components, compile the formula whole,
or take the caller's component vtrees), an `Output` (the Boolean diagram, a
counting diagram, weighted or not, or a projection of the Boolean or the
unweighted counting diagram) and the deadline the compile gives up at. It
returns `Compiled` — the diagram, its peak size and how many projected
variables it left free — or a `GiveUp` that says why it has no diagram.

| file | what it holds |
|---|---|
| `mod.rs` | `compile_cnf`, its options, result and `GiveUp`; the weighted count and the per-component streaming count the stages call. |
| `component.rs` | The job one pass runs, the checks that the vtree and the split fit the formula, and the split: each component renumbered, compiled and grafted back. |
| `step.rs` | `compile_component`, the bottom-up pass over one vtree, and its per-pass state. |
| `scope.rs` | The vtree node each clause is conjoined at, and the clause order within a node. |
| `batch.rs` | One node's clauses folded into its diagram, a batch at a time. |
| `schedule.rs` | When each level may be summed out: after which step, and after which clause within it. |
| `marginalize.rs` | Summing levels out as the pass climbs, a projection's ∃-forget, and the shield that keeps the levels a pending forget reads. |
| `minimize.rs` | The reductions the pass asks for, and the gate that picks one after each merge. |
| `sat_prune/` | SAT-based pruning of nodes no model of the formula reaches. |

## Paper sections to code

| § | Paper section | Implements |
|---|--------------|------------|
| §3 | TDD definition | `tididi/src/diagram/` — data structures |
| §3 | Determinism | `tididi/src/diagram/` — invariants |
| §4 | Apply/conjunction | `tididi/src/apply/` — conjunction and clause conjunction |
| §4 | Negation | `tididi/src/apply/negate.rs` |
| §4 | Minimization | `tididi/src/reduce/` |
| §5 | Bottom-up compilation | `src/compile/` |
| §5 | Clause to TDD | `tididi/src/apply/conjoin_clause/` |
| §5 | Formula → vtree | Vitri's constructors (`vendor/vitri/src/decompose/`), requested by `src/vtree.rs` |
| §5 | CNF and preprocessing | Vitri's `vitri::cnf` and `vitri::projection`, used directly; `src/convert.rs` converts only what crosses into the diagram's types |

## The library boundary

Three crates, each owning one thing:

- **vitri** owns the CNF: the DIMACS reader, the formula type, preprocessing and
  every vtree construction. It also vendors and builds CaDiCaL and Arjun,
  and takes FlowCutter from the GoATD crate.
- **tididi** owns the diagram: `Tdd`, `TddLevel`, `Vtree`, apply, reduce, the
  queries, and the `Engine` that runs them. Its rustdoc defines their
  representation and invariants; this compiler uses them through their public
  accessors.
- **this crate** owns the compile: `compile_cnf` and the stages that drive it.

The reduction policy the compiler applies is `src/compile/minimize.rs`,
which picks a `ReductionPlan`. [Using diagrams from the
compiler][library-integration] shows how to run a bounded compile and use its
result.

## Invariants

The diagram's structural invariants belong to the library: the
[invariant table][library-invariants] in its architecture reference numbers
them and names the checker that decides each. This crate's tests call those
checkers through the library's `testing` feature,
`tididi::test_helpers::check`.

The compiler adds one invariant, about its build: the release profile keeps
`panic = "unwind"`, as [CONTRIBUTING.md][contributing-code] requires.
`src/mem.rs`'s module doc says why the panic has to come from the call site of
the failed allocation, and `src/stage/mc/recovery.rs` catches it.

## Design pages

- [Algorithms][algorithms]: the vtree-guided bottom-up compile, the
  fixed-batch clause merge and the component graft.
- [Marginal TDD levels][marginal-tdd-levels]: levels replaced by per-node
  counts once no later step can change them, and the peak size they save.
- [Weighted marginal levels][weighted-marginal-levels]: the arithmetic a
  weighted run gets, and how to test a weighted change.
- [SAT-based pruning][sat-pruning]: dropping intermediate nodes that no
  model of the formula passes through.
- [The support-enumeration stage][support-enum-stage]: the model count
  cascade's last fallback, when it runs and why its count is sound.
- [Using diagrams from the compiler][library-integration]: a bounded
  compile on one engine, and what this crate adds to the library.

<!-- Absolute links: rustdoc renders this page as the crate's `architecture`
module (src/guide.rs), where a relative link would not resolve. -->
[algorithms]: https://github.com/Tractables/tididi-bonsai/blob/main/docs/algorithms.md
[contributing-code]: https://github.com/Tractables/tididi-bonsai/blob/main/CONTRIBUTING.md#code
[library-integration]: https://github.com/Tractables/tididi-bonsai/blob/main/docs/library-integration.md
[library-invariants]: https://github.com/Tractables/tididi/blob/main/docs/architecture.md#invariants
[marginal-tdd-levels]: https://github.com/Tractables/tididi-bonsai/blob/main/docs/marginal-tdd-levels.md
[sat-pruning]: https://github.com/Tractables/tididi-bonsai/blob/main/docs/sat-pruning.md
[support-enum-stage]: https://github.com/Tractables/tididi-bonsai/blob/main/docs/support-enum-stage.md
[weighted-marginal-levels]: https://github.com/Tractables/tididi-bonsai/blob/main/docs/weighted-marginal-levels.md
