# Evaluation

The three comparisons cover **all 1,400 MCC 2020–2026 track-1 entries**,
with **120 s, one CPU and 60 GiB per instance**. Repeated formulas share one
measurement: every tool uses the same 1,153 representative inputs, without a
solvability filter. Failed runs remain in each competition set's denominator.

[Interactive plots](https://tractables.github.io/tididi-cnf/evaluation/) ·
[Per-instance data](https://github.com/Tractables/tididi-cnf/tree/evaluation/results) ·
[Measurement details](https://github.com/Tractables/tididi-cnf/blob/evaluation/method.md) ·
[Supplementary data](https://github.com/Tractables/tididi-cnf/tree/evaluation/archive)

Plots show elapsed time horizontally, starting at 200 ms, and completed
instances vertically. Higher is better. The interactive page also offers a
logarithmic time scale.

## Bottom-up decision-diagram compilation

- **BDDs:** FORCE variable and clause orders, then balanced conjunction with
  a static variable order, with an additional CUDD sifting configuration.
- **SDDs:** a FORCE initial vtree, with search disabled (fixed) or enabled
  (dynamic).
- **tididi-cnf:** default preprocessing and portfolio vtree selection, original
  clause order, and the same saved circuits in both compiler comparisons.

BDD/SDD ordering preparation is outside the timer. If FORCE preparation fails,
BDDs retain the original variable numbering and SDD uses its default initial
tree. Their diagrams must materialize in memory; tididi-cnf also writes its
circuit. Constant true and false diagrams count as completed.

<!-- evaluation table: generated from results/mcc-track1-2020-2026-dd-compile-120s.csv -->

| Solver | MCC 2020 (200) | MCC 2021 (200) | MCC 2022 (200) | MCC 2023 (200) | MCC 2024 (200) | MCC 2025 (200) | MCC 2026 public (100) | MCC 2026 private (100) | All (1400) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tididi-cnf | **130** | **87** | **117** | **117** | **76** | **77** | **59** | **49** | **712** |
| OxiDD | 22 | 9 | 27 | 4 | 25 | 20 | 25 | 21 | 153 |
| BuDDy | 24 | 10 | 28 | 4 | 25 | 20 | 25 | 21 | 157 |
| CUDD (static) | 18 | 8 | 26 | 3 | 25 | 19 | 24 | 21 | 144 |
| Sylvan (static) | 17 | 7 | 25 | 3 | 25 | 20 | 24 | 21 | 142 |
| lib-bdd | 18 | 8 | 26 | 3 | 25 | 20 | 25 | 21 | 146 |
| SDD (fixed) | 8 | 3 | 19 | 4 | 12 | 9 | 19 | 19 | 93 |
| SDD (dynamic) | 9 | 6 | 31 | 12 | 15 | 14 | 22 | 19 | 128 |
| CUDD (sifting) | 7 | 6 | 27 | 8 | 13 | 11 | 27 | 18 | 117 |

<!-- end of evaluation table -->

## Logic-circuit compilation

All compilers must write their circuits within the time limit. A separate,
untimed checker reads the circuit, checks variable coverage and compares
available exact model counts. Failed checks count as unsolved; count agreement
alone does not prove Boolean equivalence. An UNSAT answer represents the
constant-false circuit.

D4 and GPMC are labelled by task: **compiler** here, **counter** below.

<!-- evaluation table: generated from results/mcc-track1-2020-2026-logic-compile-120s.csv -->

| Solver | MCC 2020 (200) | MCC 2021 (200) | MCC 2022 (200) | MCC 2023 (200) | MCC 2024 (200) | MCC 2025 (200) | MCC 2026 public (100) | MCC 2026 private (100) | All (1400) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tididi-cnf | **130** | **87** | **117** | **117** | **76** | **77** | **59** | **49** | **712** |
| Panini | 112 | 60 | 98 | 102 | 57 | 55 | 53 | 41 | 578 |
| D4 (compiler) | 59 | 56 | 98 | 79 | 52 | 48 | 55 | 40 | 487 |
| GPMC (compiler) | 84 | 50 | 92 | 76 | 49 | 48 | 50 | 33 | 482 |
| c2d | 79 | 44 | 66 | 57 | 36 | 30 | 44 | 31 | 387 |

<!-- end of evaluation table -->

## Model counting, two minutes

The baselines are the exact counters submitted to the 2026 competition,
running their submitted scripts. A solver must return an exact count within
the limit. Completed counts agree with the available independent results.

<!-- evaluation table: generated from results/mcc-track1-2020-2026-120s.csv -->

| Solver | MCC 2020 (200) | MCC 2021 (200) | MCC 2022 (200) | MCC 2023 (200) | MCC 2024 (200) | MCC 2025 (200) | MCC 2026 public (100) | MCC 2026 private (100) | All (1400) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tididi-cnf | **144** | 91 | 134 | 124 | **82** | 82 | 60 | **53** | 770 |
| Ganak | 142 | **102** | **141** | **129** | 81 | **87** | **61** | 49 | **792** |
| D4 (counter) | 123 | 66 | 123 | 94 | 61 | 65 | 53 | 44 | 629 |
| GPMC (counter) | 93 | 58 | 123 | 94 | 60 | 56 | 53 | 40 | 577 |
| Cara | 90 | 36 | 110 | 83 | 53 | 54 | 52 | 39 | 517 |

<!-- end of evaluation table -->

## One hour

The same counters have one hour per instance on the 100 private MCC 2026
instances. The 33 instances that no solver had previously completed were not
rerun and count as unsolved for every solver.

<!-- evaluation table: generated from results/mcc-track1-3600s.csv -->

| Solver | MCC 2026 private (100) |
| --- | ---: |
| tididi-cnf | 61 |
| Ganak | **62** |
| D4 (counter) | **62** |
| GPMC (counter) | 54 |
| Cara | 58 |

<!-- end of evaluation table -->

## Measurement setup

The compiler is built from the
[0.1.0 evaluation snapshot](https://github.com/Tractables/tididi-cnf/tree/f8acdab8ae4f5becee2c8996905b844f17331efd).
These results describe that snapshot, not subsequent source changes.

Both compiler panels use identical tididi-cnf results. Its time includes
input normalization, preprocessing, vtree selection, compilation and circuit
writing; validation follows timing. Other compilers exclude input preparation
and include process startup, parsing and compilation. Native BDD/SDD completion
and size checks are required; fixed-width counts are not exact validation.

All hosts use Xeon Platinum 8488C CPUs. Decision-diagram packages and tididi-cnf
run on `r7i.48xlarge` with 21 instances at a time. Other logic compilers run on
`r7i.24xlarge` with nine instances per compiler (45 total). Two-minute counters
run 24 instances at a time, one solver at a time; one-hour counters run eight
per solver (40 total). The 60 GiB limit is an address-space cap. Versions,
commands and resource details are in the linked measurement details.
