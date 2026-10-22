# Evaluation data

Per-instance results for [tididi-cnf](https://github.com/Tractables/tididi-cnf).
The compiler source and concise comparison are on `main`; this branch holds
measurement data.

- [Current comparisons](results/): decision-diagram compilation, logic-circuit
  compilation and model counting on all MCC 2020–2026 entries, plus one-hour
  counting on the private MCC 2026 set.
- [Method](method.md): measured versions, timing and validation.
- [Supplementary datasets](archive/): selected CNF2BDD and MCC cohorts.

Each CSV records the competition set, instance, solver, status and elapsed
time. Failed runs remain in the denominator. `results/datasets.json` lists the
four comparisons shown on the interactive page. Site builds pin this branch
to a commit, so published source and data remain reproducible.

This material is based upon work supported by the Defense Advanced Research
Projects Agency (DARPA) under Agreement No. HR0011269E224.
