# Tasks

A run answers one of five tasks, its *mode*. This page says what each one
computes, how a run picks one, and which flags each accepts. `tididi` in the
commands is the built binary, `target/release/tididi`.

## The five modes

A formula `p cnf n m` has *n* variables, numbered 1 to *n*. An assignment
gives each of them a value, and a model is an assignment that satisfies every
clause.

| mode | computes | arithmetic | answer line |
|---|---|---|---|
| `mc` | the number of models | exact integer | `model count: 22` |
| `wmc` | the sum over models of the product of their literals' weights | double precision | `weighted count: 3.2999999999999985e0` |
| `pmc` | the number of assignments to the show variables that some model extends | exact integer | `projected model count: 3` |
| `pwmc` | the sum, over those assignments to the show variables, of the product of their literals' weights | exact rational | `projected weighted count: 22/25` |
| `compile` | a Tree Decision Diagram of the formula over all *n* variables, and its model count | exact integer | `model count: 22` |

Some details of what each count ranges over are easy to miss:

- **Every declared variable counts.** A variable that `p cnf` declares but no
  clause mentions doubles the model count: `p cnf 4 2` over the clauses
  `1 2 0` and `-1 -2 0` has 8 models, not 2.
- **A literal without a weight line weighs `1 - w`** when the other literal
  of its variable has the weight `w`, strictly between 0 and 1, as the
  competition's format defines. Any other weight on one literal alone is
  refused, and a variable with no weight line weighs 1 either way.
  [`input.md`](input.md#weights) has an example of each.
- **The projected weighted count reads only the show variables' weights.**
  A weight on any other variable is ignored.
- **The show set can be empty.** `c p show 0` projects onto no variables, so
  the projected count is 1 for a satisfiable formula and 0 otherwise.
- **A projected mode without a show line** counts over every variable:
  `--mode pmc` on a formula with no `c p show` line answers its model count.
- **The weighted model count is not exact.** It is computed with logarithms
  in double precision, so a count far beyond the range of a floating-point
  number still prints, and its last digits carry rounding error. The
  projected weighted count, and every unweighted count, is exact.
- **Weights can be negative**, so a weighted count can be negative or zero on
  a satisfiable formula. [`output.md`](output.md#the-competitions-output-format)
  shows how the competition's format spells both.

## How a run picks its mode

The instance's header lines declare a mode, as in the model counting
competition's format ([`input.md`](input.md#header-lines)):

- a `c t mc`, `c t wmc`, `c t pmc` or `c t pwmc` line names it;
- a `c p show` line makes the count projected: `c t mc` with a show line is
  `pmc`, and `c t wmc` with one is `pwmc`. A show line with no `c t` line is
  `pmc`;
- an instance with neither is compiled: mode `compile`.

`c p weight` lines declare nothing on their own. A formula with weights and
no `c t` line is compiled, and its weights do not count.

`--mode <compile|mc|wmc|pmc|pwmc>` names the mode on the command line. With no
header, it decides. With a header, it must agree: a mode the header
contradicts is refused rather than answered, and a lone `c p show` line agrees
with `pmc` and `pwmc` only. `--mode mc` and `--mode pmc` also refuse an
instance with `c p weight` lines, which they would ignore, and name the
weighted modes `wmc` and `pwmc` instead; a run that the header alone makes
`mc` or `pmc` reads past the weights. `--mode compile` ignores `c p weight`
lines, since a circuit has no use for them.
`tests/data/mcc_mc_sat.cnf` declares `c t mc`:

```sh
tididi --mode compile tests/data/mcc_mc_sat.cnf
```

Stderr:

```text
[input] reading tests/data/mcc_mc_sat.cnf
error: mode compile contradicts the instance's `c t mc` line, which declares mode mc. Leave the mode unset to answer what the header declares.
```

The run exits with status 2 and prints nothing on stdout.

## Flags each mode accepts

A flag the mode would ignore is refused, with status 2 and a message that
names the flag and what made the run that mode. So is a flag given twice,
whose second value would otherwise replace the first.

| flag | `compile` | `mc` | `wmc` | `pmc` | `pwmc` |
|---|---|---|---|---|---|
| `--vtree <spec>` | yes | yes | refused | yes | yes |
| `--preprocess <none\|full>` | yes | yes | yes | refused | refused |
| `--save-tdd <file>` | yes | refused | refused | refused | refused |

`--output-format`, `--stats`, `--time-limit-ms` and `--mem-limit-mb` apply in
every mode.

- **`--save-tdd`** only makes sense for a compile: a counting run compiles a
  preprocessed formula whose diagram does not carry the reported count.
- **`--preprocess none`** turns off vitri's simplification of the formula
  before the compile; `full`, the default, runs it. The counting cascade of
  `mc` and `wmc` runs Arjun's reduction either way. The projected counts
  always simplify, so they refuse the flag.
- **`--vtree`** names the vtree to compile on.

## Vtrees

A vtree is a binary tree whose leaves are the formula's variables, and the
diagram's shape follows it: on the same formula, one vtree can compile in a
second and another not finish. Without `--vtree`, the run chooses:

- `compile`, `pmc` and `pwmc` build vitri's `portfolio`, which constructs
  several vtrees for the formula and keeps the one that scores best. Under
  `compile`, a formula that splits into independent parts and has at least
  1000 variables and 4.5 clauses per variable gets vitri's force-directed
  `force` tree on every part instead. The portfolio scores trees without
  compiling on them, and on some formulas the diagram outgrows the memory
  limit on its pick while another tree compiles in a small fraction of it.
  When a `compile` runs out of memory with time left, it compiles once more
  on vitri's hypergraph bisection, `hypergraph-bisect:imbalance=0.40`, unless
  the portfolio's pick already was a `hypergraph-bisect` tree; the record's
  `answered_by` is then `bisect-retry`. The retry builds its tree in the time
  left, and one that cannot finish the tree gives up. Nothing follows the
  retry.
- `mc` and `wmc` run the counting cascade: vitri's Arjun preprocessing reduces
  the formula, the portfolio picks a vtree for what is left, and if that
  compile gives up, further stages follow. The record's `answered_by`
  ([`stats.md`](stats.md)) says which stage produced a model count.
- Under `mc`, a compile of the reduced formula that stalls is given up early.
  The compile works up the vtree one node at a time, and with a time limit
  each node's step may run for a sixth of it, as long as at least 50 steps,
  that one included, are still to come in its part of the formula. A step
  that runs longer ends the compile, and the same reduced formula is compiled
  again, to the end of the run, on a vtree from vitri's portfolio restricted
  to `goatd-primal`, a construction the portfolio otherwise leaves out. The
  record's `answered_by` is then `stall-recompile`, and its `vtree_candidate`
  names the tree. A recompile that gives up leaves the formula to the stages
  that follow any failed compile. Without a time limit, no step has a limit
  of its own. `wmc` has no such step limit.

With `--vtree <spec>`, the run compiles on that construction instead, and a
`compile` that gives up on it has no retry. Under `mc` that also skips the
cascade: the formula is simplified but not reduced by
Arjun, and each of its independent parts compiles on a vtree of that
construction. `wmc` has the cascade only and refuses the flag.
`tididi --help` lists every spec and its parameters, and
[vitri's vtree guide](https://github.com/Tractables/vitri/blob/main/docs/vtrees.md)
explains the constructions. `balanced`, `linear`, `reverse-linear` and
`random` ignore the formula's structure; they are there for comparison and
compile far slower than `portfolio` on most formulas.
