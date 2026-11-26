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
{"mode":"mc","exit_reason":"solved","variables":6,"clauses":4,"vtree_spec":"portfolio","vtree_candidate":"force","answered_vtree":"force","satisfiable":true,"model_count":"22","peak_pairs":6,"answered_by":"compile","reduction":"used","preprocess_ms":1,"arjun_ms":4,"vtree_ms":14,"compile_ms":1,"cascade_ms":0,"backbone_ms":0,"backbone_found":0,"backbone_probes":2}
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
| `vtree_spec` | string | The vtree construction asked for: the `--vtree` spec, or `portfolio` without the flag. Absent when the input is a vitri bundle, which brings its vtree. | all |
| `vtree_candidate` | string | A vtree construction the run used, by its spec. An `mc` or `wmc` answer names the winner of the run's last portfolio selection. Under `compile` it is the tree the last compile ran on, named as that compile starts, so a run that gives up carries it too: the portfolio's winner, or `hypergraph-bisect:imbalance=0.40` once the retry on it ran. Under `mc` it is the tree the first compile started on, named as it starts, so a run that gives up names it too, and a fallback that answers does not replace it. | `compile`, `mc`, `wmc`, when a portfolio ran |
| `answered_vtree` | string | The tree the stage in `answered_by` compiled on, by its spec: `vtree_candidate` when the first compile answered, `hypergraph-bisect:imbalance=0.40` for `bisect-retry` and `no-sbva-retry`, and the portfolio's winner for its own tree under `unreduced-retry` and `monotone-reduction`. Absent for `preprocessing`, `support-enumeration` and `shannon-split`, which answer on no one tree. | `mc` without `--vtree`, once answered |
| `satisfiable` | boolean | Whether the formula has a model. A weighted count can be 0 on a satisfiable formula. | all, once known |
| `model_count` | string | The count, in decimal. | `compile`, `mc`, `pmc` |
| `weighted_count` | string | The count as the answer line prints it: `<n>/<d>` for `pwmc`, `<mantissa>e<exponent>` for `wmc`. | `wmc`, `pwmc` |
| `weighted_count_log10` | number | The base-10 logarithm of the count's absolute value. Absent for a count of 0. | `wmc` |
| `weighted_count_sign` | integer | The count's sign: `-1`, `0` or `1`. | `wmc` |
| `tdd_nodes` | integer | Nodes in the compiled diagram. | `compile` |
| `tdd_pairs` | integer | Pairs in the compiled diagram, the measure of its size. | `compile` |
| `tdd_width` | integer | The most nodes the compiled diagram has at one vtree node. | `compile` |
| `peak_pairs` | integer | The largest pair count sampled after a compile step's fold, before its final marginalization pass; the maximum over components, not a transient allocation peak ([measurement](marginal-tdd-levels.md#during-the-compile)). Under `compile`, the compile that answered. | `compile`, `mc` |
| `answered_by` | string | The stage that produced the count: `compile`; `preprocessing`, when simplification settled it; or one of the stages that take over from a compile that gave up: under `mc`, `bisect-retry`, `support-enumeration`, `no-sbva-retry`, `unreduced-retry`, `monotone-reduction` or `shannon-split`, and under `compile`, `bisect-retry` too ([`tasks.md`](tasks.md#vtrees)). | `compile`, `mc` |
| `reduction` | string | `used` when the count went through the formula Arjun reduced, `unavailable` when Arjun gave up or its result was not kept. Absent when Arjun did not run. | `mc` |
| `preprocess_ms` | integer | Time simplifying the formula; under `mc`, without Arjun, which has its own key. | `compile`, `mc` |
| `arjun_ms` | integer | Time in Arjun's reduction. | `mc` |
| `vtree_ms` | integer | Time building vtrees, over the whole run. | all, once a vtree was built |
| `learned_ms` | integer | Time vitri spent on its learned tree in the portfolio build that chose `vtree_candidate`, whether it took the tree or not; part of `vtree_ms`. When it took the tree, `vtree_candidate` is `model-order`. | `compile`, `mc`, `wmc`, when that build built the learned tree |
| `compile_ms` | integer | Time compiling, not counting the keys above; under `mc` and `compile` it includes the stages that take over from a compile that gave up, and under `pmc` and `pwmc` every stage that did not rewrite the formula, enumeration included. 0 when preprocessing settled a `compile` or `mc` count. | `compile`, `mc`, `pmc`, `pwmc` |
| `cascade_ms` | integer | Time from the count's first compile to the answer, vtree construction and the stages that take over from a compile that gave up included. Absent when preprocessing settled the count. | `mc` |
| `backbone_ms` | integer | Time finding the formula's backbone, the literals every model shares. | `compile`, `mc` |
| `backbone_found` | integer | Backbone literals found. | `compile`, `mc` |
| `backbone_probes` | integer | SAT calls the backbone search made. | `compile`, `mc` |
| `show_vars` | integer | Variables in the show set of the formula the count was answered on, after preprocessing. | `pmc`, `pwmc`, with a show set |
