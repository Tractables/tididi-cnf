# Evaluation

The [evaluation in the README](../README.md#evaluation) compares tididi-bonsai
with the exact model counters entered in the 2026
[Model Counting Competition](https://mccompetition.org/), on the instances of
the competition's track 1, unweighted model counting, from 2025 and 2026. This
page says how it was measured. The result of every run is in
[`evaluation/mcc-track1-120s.csv`](evaluation/mcc-track1-120s.csv): the set,
the instance, the solver, whether it printed a count in time and after how
many seconds.

## Instances

Track 1 of the 2025 competition has 200 instances. The 2026 competition
published 100 instances in advance and scored the entries on 100 others,
which it published afterwards; the tables call them the public and the
private set. Every set is counted in full. Ten instances of 2025 and the 2026
public set repeat another instance there, identical but for a comment line;
each formula was run once, and its repeat takes its result.

## Solvers

- tididi-bonsai 0.1.0, as
  `tididi --mode mc --time-limit-ms 120000 --mem-limit-mb 61440 <instance>`:
  its defaults, told the time and memory the run has
  ([time and memory limits](limits.md)).
- The 2026 competition's entries in its exact category, as submitted, each
  run by its own script for the competition's two-minute track: Ganak (in the
  two-minute configuration it entered), d4, GPMC and Cara. The entries in the
  approximate and heuristic categories do not promise exact counts and are
  left out.

## Machine and limits

All runs were on an AWS `r7i.24xlarge` machine (48 cores, 768 GiB), 24 at a
time, one solver after another. Each run had 120 s of wall-clock time and a
60 GiB address-space limit; the competition allowed 120 s of CPU time and
62,000 MiB. An instance is solved when the solver printed its count within the
time. Every count agrees with the count of each other solver that finished
the instance and, on the private set, with the competition's.

## One hour

Given an hour per instance (3600 s, 48 runs at a time on the same kind of
machine, a 30,000 MiB address-space limit), tididi-bonsai solves 120 of the
200 instances of 2025, 69 of the 100 public instances of 2026 and 61 of the
100 private ones.

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
compare one to one; on ours, tididi-bonsai 0.1.0 solves 53 of these instances
in 120 s and 61 in an hour.
