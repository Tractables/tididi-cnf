# The stats record

`--stats <path>` writes one JSON object that describes the run, and
`--stats -` writes it to stdout as a single line after the answer. It is the
part of a run's output that a program should read: stderr is progress for
people ([`output.md`](output.md#stderr)). `tididi` in the commands is the
built binary, `target/release/tididi`.

```sh
tididi --stats - tests/data/mcc_mc_sat.cnf
```

```text
model count: 22
{"mode":"mc","exit_reason":"solved","variables":6,"clauses":4,"vtree_spec":"portfolio","vtree_candidate":"force","satisfiable":true,"model_count":"22","peak_pairs":6,"answered_by":"compile","reduction":"used","preprocess_ms":1,"arjun_ms":4,"vtree_ms":14,"compile_ms":1,"cascade_ms":0,"backbone_ms":0,"backbone_found":0,"backbone_probes":2,"sat_prune_solver_setup_us":97}
```

## Conventions

- **Keys are `snake_case` and stable.** A key is present only when it applies
  to the run's mode and to the way the run went; there are no nulls.
- **Counts are strings.** `model_count` is a decimal string, since an exact
  count is often too large for a JSON number, and `weighted_count` is the
  string the answer line prints. Every other value is a JSON number, string or
  boolean.
- **Times** ending `_ms` are milliseconds of wall-clock time, and times ending
  `_us` are microseconds.
- **A run that gives up still writes a record,** with what it knew when it
  stopped ([`limits.md`](limits.md#giving-up)). A run refused before its mode
  starts, such as a command line the parser cannot read, writes none, and
  neither does a process killed from outside.
- **A record that cannot be written fails the run** with exit status 1, after
  the answer is printed ([`output.md`](output.md#exit-status)).

## Keys

The last column says in which modes a key can appear.

| key | type | meaning | modes |
|---|---|---|---|
| `mode` | string | `compile`, `mc`, `wmc`, `pmc` or `pwmc` ([`tasks.md`](tasks.md)). | all |
| `exit_reason` | string | `solved`, or why the run ended without an answer: `usage`, `environment`, `stage`, `vtree-spec`, `input`, `artifact` or `incomplete` ([`output.md`](output.md#exit-status)). | all |
| `variables` | integer | Variables the `p cnf` line declares. | all |
| `clauses` | integer | Clauses read from the file, less those every assignment satisfies ([`input.md`](input.md#dimacs)). | all |
| `vtree_spec` | string | The vtree construction asked for: the `--vtree` spec, or `portfolio` without the flag. | all |
| `vtree_candidate` | string | The construction that won the portfolio's selection. Under `compile` it is the tree the last compile ran on, named as that compile starts, so a run that gives up carries it too: the portfolio's winner, or `hypergraph-bisect:imbalance=0.40` once the retry on it ran. Under `mc` it is named as the primary compile starts, so a run that gives up names the primary's tree; an answer names the winner of the last selection. | `compile`, `mc`, `wmc`, when a portfolio ran |
| `satisfiable` | boolean | Whether the formula has a model. A weighted count can be 0 on a satisfiable formula. | all, once known |
| `model_count` | string | The count, in decimal. | `compile`, `mc`, `pmc` |
| `weighted_count` | string | The count as the answer line prints it: `<n>/<d>` for `pwmc`, `<mantissa>e<exponent>` for `wmc`. | `wmc`, `pwmc` |
| `weighted_count_log10` | number | The base-10 logarithm of the count's absolute value. Absent for a count of 0. | `wmc` |
| `weighted_count_sign` | integer | The count's sign: `-1`, `0` or `1`. | `wmc` |
| `tdd_nodes` | integer | Nodes in the compiled diagram. | `compile` |
| `tdd_pairs` | integer | Pairs in the compiled diagram, the measure of its size. | `compile` |
| `tdd_width` | integer | The most nodes the compiled diagram has at one vtree node. | `compile` |
| `peak_pairs` | integer | The largest diagram the compile held at once, in pairs; under `compile`, the compile that answered. | `compile`, `mc` |
| `answered_by` | string | The stage that produced the count: `compile`; `preprocessing`, when simplification settled it; or one of the stages that take over from a compile that gave up: under `mc`, `support-enumeration`, `no-sbva-retry`, `unreduced-retry` or `shannon-split`, and under `compile`, `bisect-retry` ([`tasks.md`](tasks.md#vtrees)). | `compile`, `mc` |
| `reduction` | string | `used` when the count went through the formula Arjun reduced, `unavailable` when Arjun gave up or its result was not kept. Absent when Arjun did not run. | `mc` |
| `preprocess_ms` | integer | Time simplifying the formula; under `mc`, without Arjun, which has its own key. | `compile`, `mc` |
| `arjun_ms` | integer | Time in Arjun's reduction. | `mc` |
| `vtree_ms` | integer | Time building vtrees, over the whole run. | all, once a vtree was built |
| `compile_ms` | integer | Time compiling, not counting the keys above; under `mc` and `compile` it includes the stages that take over from a compile that gave up, and under `pmc` and `pwmc` every stage that did not rewrite the formula, enumeration included. 0 when preprocessing settled a `compile` or `mc` count. | `compile`, `mc`, `pmc`, `pwmc` |
| `cascade_ms` | integer | Time from the count's first compile to the answer, vtree construction and the stages that take over from a compile that gave up included. Absent when preprocessing settled the count. | `mc` |
| `backbone_ms` | integer | Time finding the formula's backbone, the literals every model shares. | `compile`, `mc` |
| `backbone_found` | integer | Backbone literals found. | `compile`, `mc` |
| `backbone_probes` | integer | SAT calls the backbone search made. | `compile`, `mc` |
| `show_vars` | integer | Variables in the show set of the formula the count was answered on, after preprocessing. | `pmc`, `pwmc`, with a show set |
| `sat_prune` | string | `off` when `TIDIDI_SAT_PRUNE=off` turned SAT pruning off for the run, a diagnostic for paired measurements ([`env.md`](env.md#tididi_sat_prune)); no `sat_prune_…` key appears then. Absent when pruning is on, the default. | `compile`, `mc`, `pmc`, `pwmc` |
| `sat_prune_disabled_by` | string | Why SAT pruning stopped before the compile ended: `empty-passes`, after consecutive passes removed nothing; `low-kill-rate`, when it removed too few of the nodes it checked; `time-share`, when it spent too much of the time limit, in one component or over all of a compile's components together. | all but `wmc` |

## SAT pruning tallies

During a compile, when the diagram has grown past a size threshold, a SAT
solver that holds the formula checks the diagram's nodes, and the nodes no
model passes through are removed. Such a *pass* runs in every mode but `wmc`,
unless `TIDIDI_SAT_PRUNE=off` turned it off;
[`sat-pruning.md`](sat-pruning.md) describes it. Its
tallies, summed over every compile of the run, appear as
`sat_prune_<name>`, each only when it is not 0.

| `sat_prune_…` | counts |
|---|---|
| `passes_triggered` | passes started |
| `pairs_inspected` | the diagram's size in pairs at the start of each pass, summed over passes |
| `probes_attempted` | nodes checked one at a time |
| `probes_unsat` | nodes a check proved dead |
| `probes_unknown` | checks that ran out of budget; the node is kept |
| `nodes_pruned` | nodes found dead, directly or because every pair of theirs leads to a dead node |
| `passes_dead_root` | passes that found the whole diagram dead |
| `passes_rewritten` | passes that removed dead nodes and rebuilt the diagram |
| `pairs_filtered` | pairs removed by those rebuilds |
| `rewrite_us` | time rebuilding |
| `sat_calls` | calls to the SAT solver |
| `solver_setup_us` | time building the solver and loading the formula |
| `probe_us` | time in the SAT solver |
| `encode_us` | time encoding the diagram as clauses for the solver |
| `passes_aborted_encode_budget` | passes stopped because encoding ran out of budget |
| `passes_encode_prefix` | passes that ran out of encoding budget and checked the part already encoded |
| `prefix_nodes_skipped` | nodes left unchecked because they were beyond that part |
| `zero_count_kills` | nodes found dead because their model count is 0 |
| `count_order_us` | time counting models per node, to order the checks |
| `rewrite_propagated_kills` | nodes that lost every pair during a rebuild |
| `nodes_skipped_witness` | checks skipped because an earlier solution already passed through the node |
| `witness_val_queries` | lookups of a solution's value at a node |
| `allsat_global_attempted` | solver calls that enumerate solutions of the whole diagram at once |
| `allsat_global_sat` | those that found a solution |
| `allsat_global_unsat` | those that found no further solution, ending the enumeration |
| `allsat_global_unknown` | those that ran out of budget |
| `allsat_witnesses_harvested` | nodes first shown alive by an enumerated solution |
| `allsat_kills` | nodes found dead because a finished enumeration never passed through them |
| `allsat_us` | time in the enumeration's solver calls, part of `probe_us` |
| `dead_child_reorders` | vtree levels whose checks were reordered to put nodes with dead children first |

The list is declared once, in `SatPruneStats`, from which the record's keys
are made.
