# Evaluation

The [evaluation in the README](../README.md#evaluation) compares tididi-cnf
with the exact model counters entered in the 2026
[Model Counting Competition](https://mccompetition.org/), and with four d-DNNF
compilers, on the instances of the competition's track 1, unweighted model
counting, from 2025 and 2026. This page says how it was measured. The result
of every run is in
[`evaluation/mcc-track1-120s.csv`](evaluation/mcc-track1-120s.csv), for an
hour per instance in
[`evaluation/mcc-track1-3600s.csv`](evaluation/mcc-track1-3600s.csv) and for
compilation in
[`evaluation/mcc-track1-compile-120s.csv`](evaluation/mcc-track1-compile-120s.csv):
the set, the instance, the solver, whether it finished in time and after how
many seconds. The
[evaluation page](https://tractables.github.io/tididi-cnf/evaluation/)
shows these runs by set, by pair of solvers and by instance; it reads the
files of runs that [`evaluation/datasets.json`](evaluation/datasets.json)
lists, and its source is beside them.

The measurements use the [0.1.0 evaluation snapshot](https://github.com/Tractables/tididi-cnf/tree/f8acdab8ae4f5becee2c8996905b844f17331efd),
including the dependency revisions recorded there.

## Instances

Track 1 of the 2025 competition has 200 instances. The 2026 competition
published 100 instances in advance and scored the entries on 100 others,
which it published afterwards; the tables call them the public and the
private set. Every set is counted in full. Ten instances of 2025 and the 2026
public set repeat another instance there, identical but for a comment line;
each formula was run once, and its repeat takes its result.

## Solvers

- tididi-cnf 0.1.0, as
  `tididi --mode mc --time-limit-ms 120000 --mem-limit-mb 61440 <instance>`:
  its defaults, told the time and memory the run has
  ([time and memory limits](limits.md)).
- The 2026 competition's entries in its exact category, as submitted, each
  run by its own script for the track of the run: Ganak, which entered one
  configuration for two minutes and another for an hour, and d4, GPMC and
  Cara, which entered one script for both. The entries in the approximate and
  heuristic categories do not promise exact counts and are left out.

## Machine and limits

Every run was on an AWS `r7i.24xlarge` machine (48 cores, 768 GiB). The
two-minute counts ran 24 at a time, one solver after another. Each run had
120 s of wall-clock time and a 60 GiB address-space limit; the competition
allowed 120 s of CPU time and 62,000 MiB. An instance is solved when the
solver printed its count within the time. Every count agrees with the count of
each other solver that finished the instance and, on the private set, with the
competition's.

## One hour

The competition's main track, track 1, allows an hour per instance. On the
100 private instances, the five solvers ran for an hour each on the same kind
of machine under the same limits, side by side: 8 runs of each at a time, 40
in all, so that every solver ran under the same load. An instance that no
entry in the exact category solved in either track of the competition, and
that tididi-cnf had not solved in an hour before, was not run again and
counts as unsolved for every solver; that leaves 67 of the 100 to run.

<!-- evaluation table: generated from docs/evaluation/mcc-track1-3600s.csv -->

| Solver | MCC 2026 private (100) |
| --- | ---: |
| tididi-cnf | 61 |
| Ganak | **62** |
| d4 | **62** |
| GPMC | 54 |
| Cara | 58 |

<!-- end of evaluation table -->

Given an hour on its own (48 runs at a time, a 30,000 MiB address-space
limit), tididi-cnf solves 120 of the 200 instances of 2025 and 69 of the
100 public instances of 2026.

## Compilation

With `--mode compile`, tididi-cnf writes the diagram it compiles, which
the tididi library reads back and queries. Four d-DNNF compilers, whose
circuits can be queried in the same ways, ran beside it on the same 400
instances, two minutes each, side by side (9 runs of each at a time, 45 in
all) on the same kind of machine under the same memory limit:

- tididi-cnf 0.1.0, as
  `tididi --mode compile --save-tdd <file> --time-limit-ms 120000 --mem-limit-mb 61440 <instance>`,
  on the instance without its `c t mc` line, since compilation takes no task.
- [Panini](https://github.com/meelgroup/KCBox),
  [D4v2](https://github.com/crillab/d4v2) and
  [GPMC](https://github.com/System-Verification-Lab/GPMC), built from their
  repositories' main branches, GPMC in its d-DNNF mode with variable
  elimination off so that its circuit keeps every variable, and
  [c2d](http://reasoning.cs.ucla.edu/c2d/) 2.20.

An instance is compiled when the compiler wrote its circuit within the time
and a separate checker, reading the circuit back, found that it mentions every
variable and that its model count is the one the compiler reported. D4v2
reports no count, so its time is its own run, without the check. On one
instance of 2025, Panini's circuit and count differ from the count of
tididi-cnf and of Ganak; it counts as not compiled there.

<!-- evaluation table: generated from docs/evaluation/mcc-track1-compile-120s.csv -->

| Solver | MCC 2025 (200) | MCC 2026 public (100) | MCC 2026 private (100) | All (400) |
| --- | ---: | ---: | ---: | ---: |
| tididi-cnf | **73** | **59** | **49** | **181** |
| Panini | 55 | 53 | 41 | 149 |
| D4v2 | 48 | 55 | 40 | 143 |
| GPMC | 48 | 50 | 33 | 131 |
| c2d | 30 | 44 | 31 | 105 |

<!-- end of evaluation table -->

## The competition's own results

The 2026 competition ran its entries on its own cluster and published the
[results](https://mccompetition.org/assets/files/2026/MC2026_awards.pdf). An
earlier version of this compiler took part as TiDiDi. On the 100 private
instances, the entries in the exact category solved, in two minutes (track
1F) and in an hour (track 1):

| Solver | 120 s | 3600 s |
| --- | ---: | ---: |
| d4 | 42 | **63** |
| Ganak | **48** | 62 |
| Cara | 38 | 59 |
| GPMC | 38 | 57 |
| TiDiDi | 46 | 56 |

Its machines and ours differ, so these numbers and the ones above do not
compare one to one: on ours, d4 solves one instance fewer in an hour, Cara
one and GPMC three, and tididi-cnf 0.1.0 solves 53 of these instances in
120 s and 61 in an hour.
