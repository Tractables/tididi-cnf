# Supplementary datasets

These subsets complement the full-corpus comparisons in [results](../results/).
They are not additional full-MCC evaluations. Compiler snapshots differ;
compare tools within each dataset.

| Dataset | Inputs | Limit | Data |
| --- | --- | --- | --- |
| CNF2BDD-Bench MCC family | 231 selected, preprocessed MCC formulas | 600 s | [CSV](cnf2bdd-2026-09-600s.csv) |
| Original MCC cohort | 635 selected, unpreprocessed formulas | 600 s | [CSV](mcc-cohort-2026-09-600s.csv) |
| MCC 2025–2026 compilation | All 400 entries, original representatives | 120 s | [CSV](mcc-track1-compile-120s.csv) |
| MCC 2025–2026 counting | All 400 entries, original representatives | 120 s | [CSV](mcc-track1-120s.csv) |

## Decision-diagram subsets

CNF2BDD-Bench uses every formula in the model-counting family of the
[OxiDD STTT artifact](https://zenodo.org/records/13923989): a D4-runtime-filtered
MCC 2020–2023 subset, preprocessed with pmc and ordered with MINCE.
The 635-formula cohort uses original MCC 2020–2026 CNFs from a solvable working
list with exported TDD inputs, including 28 trivial formulas. Packages use
FORCE orders, with one original-numbering fallback; preparation is untimed.

The BDD package revisions and balanced static conjunction match
[the primary method](../method.md#decision-diagram-packages). The September
2026 compiler snapshot uses default simplification and vtree selection.
Its binary SHA-256 is
`25ce614740d85433e4e591f452534391b3fe24f189eb3742392a914d93094d04`.

Each invocation has one CPU, 600 s wall time, a 60 GiB internal budget,
64 GiB address-space cap and 1 GiB stack. CNF2BDD runs 32 instances at a time
on a shared `r7i.48xlarge`, about 75 total jobs. The original-CNF cohort runs
535 formulas on that host at 40 concurrent jobs and 100 on a dedicated
`r7i.24xlarge` at 16. All tools for a formula use the same host; both use
Xeon Platinum 8488C CPUs. Cross-host timing is not calibrated.

Time includes process startup, parsing and in-memory compilation; TDD time
also includes its simplification, tree construction and counting. File output
is not required. Successful exit and diagram size establish completion.
Size agreement is not independent count validation or an equivalence proof.
Each tool/formula has one measurement.

## MCC 2025–2026 subsets

These comparisons use the [primary method's](../method.md) logic compilers,
counting submissions and resource limits, with their original representative
inputs. Ten repeated entries share their twins' results. TDD compilation uses
the original saved-circuit measurements; it is a separate series from the
full-corpus TDD measurements.

This material is based upon work supported by the Defense Advanced Research
Projects Agency (DARPA) under Agreement No. HR0011269E224.
