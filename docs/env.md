# Environment variables

`tididi` runs with no environment variables set, and none is needed. It reads
two of its own, and the `VITRI_*` variables of vitri, the library that
preprocesses the formula and builds its vtrees. Every one of them is checked
when the run starts: a value that does not parse, or a setting the run cannot
honour, stops the run with exit status 2 before any work, with a message that
names the variable.

The library's `count` entry uses the same validation and vitri settings as
the binary, including the refusals below. Every `count` reads them afresh
when it starts, so a setting changed between two calls takes effect, or is
refused, in the second. Preprocessing controls hold for the whole run;
construction controls are read by each construction, `vtree::build_vtree`'s
included. The library takes its time limit from `Limits`, while
`TIDIDI_TIME_LIMIT_MS` is a command-line default. `TIDIDI_SAT_PRUNE` is read
by the binary alone; the library's `count` always prunes.

## `TIDIDI_TIME_LIMIT_MS`

The time limit in milliseconds, when `--time-limit-ms` is not given; the flag
wins when both are. Its value must be a whole number, even when the flag is
given, since a limit that did not parse would otherwise leave the run with no
limit at all:

```sh
TIDIDI_TIME_LIMIT_MS=abc tididi --time-limit-ms 5000 tests/data/mcc_mc_sat.cnf
```

Stderr:

```text
error: environment variable TIDIDI_TIME_LIMIT_MS: "abc" is not a whole number of milliseconds
```

[`limits.md`](limits.md#time) says what the limit covers.

## `TIDIDI_SAT_PRUNE`

`off` turns off SAT pruning, the pass that removes diagram nodes no model
passes through during a compile
([`sat-pruning.md`](sat-pruning.md)); `on` leaves it on,
as it is without the variable. It is a diagnostic for paired measurements of
the same run with and without the pass: pruning never changes the answer, so
a pair differs only in time and in diagram sizes. With `off` the record says
`"sat_prune":"off"` and carries none of the pass's tallies
([`stats.md`](stats.md)).

Any other value is refused. The variable applies to the `compile`, `mc`,
`pmc` and `pwmc` modes; the weighted count's compiles never prune, so a `wmc`
run refuses it, whichever value it has:

```sh
TIDIDI_SAT_PRUNE=off tididi --mode wmc tests/data/wmc_header_tiny_sat.cnf
```

Stderr:

```text
[input] reading tests/data/wmc_header_tiny_sat.cnf
error: TIDIDI_SAT_PRUNE is set, but it applies to the compile, mc, pmc and pwmc modes, and --mode wmc makes this run a weighted count (mode wmc), whose compiles never run SAT pruning. Unset TIDIDI_SAT_PRUNE.
```

## vitri's variables

vitri's `VITRI_*` variables tune its preprocessing and its vtree
constructions, and
[vitri's catalog](https://github.com/Tractables/vitri/blob/main/docs/env.md)
lists them. `tididi` passes them through, and they reach every run that
preprocesses or builds a vtree, in every mode, with two exceptions it refuses
whenever they are set:

- **`VITRI_BUDGET_MS`.** `tididi` gives vitri its budget from the run's own
  time limit, so this variable would never take effect. Pass the limit as
  `--time-limit-ms` instead.
- **`VITRI_ARJUN_EXPORT_LEARNED_CLAUSES`.** It asks vitri for clauses that
  `tididi` does not use.

One setting is overridden by design. With `VITRI_ARJUN_SBVA` on, its
default, Arjun adds variables to shorten the formula; on a formula that looks
like a graph colouring, a model count whose first compile gives up retries
with a second reduction that has it off. The record's `answered_by` is
`no-sbva-retry` when that retry produced the count ([`stats.md`](stats.md)).

vitri's catalog also lists the variables that act when `tididi` is built
rather than when it runs, such as the choice of C++ compiler.
