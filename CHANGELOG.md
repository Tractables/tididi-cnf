# Changelog

## Unreleased

- First public release: a command-line compiler and a Rust library that compile a DIMACS CNF into a Tree Decision Diagram (TDD) with the [tididi](https://github.com/Tractables/tididi) library, after CNF preprocessing and vtree construction by [vitri](https://github.com/Tractables/vitri).
- Count models (MC), projected models (PMC) and projected weighted models (PWMC) exactly, and weighted models (WMC) in double-precision log arithmetic.
- Compile a CNF into a TDD and write the circuit to a file.
- Build a formula from clauses held as data, with `formula_from_clauses`: clauses of DIMACS integers, the show set and the literal weights, for a program that would otherwise write DIMACS text.
- Read literal weights and projection sets from the header lines of the model counting competition's instance format, so an instance can name its own task. A literal without a weight line weighs as that format defines: `1 - w` when the other literal of its variable weighs `w` strictly between 0 and 1, and 1 when neither has a line; any other lone weight is refused.
- Print an answer in the model counting competition's output format on request, besides the plain one-line answer.
- Prune an intermediate diagram of nodes that no model of the formula passes through, with SAT calls against the formula.
- Run within a wall-clock limit and a memory limit. A model count whose compile runs out of memory splits the formula on a variable and carries on rather than aborting.
- Exit with a non-zero status whenever a run ends without an answer: a give-up in any task, or a refused input or setting.
- Refuse a setting the run would ignore, naming the setting and what it needs, instead of dropping it silently.
- Build and run on Linux. The compile is single-threaded; parallelism comes from running instances side by side.
- Attach a Linux x86-64 archive of the `tididi` binary to each release, with the licence texts of the components it links.
- Count from C and Python: `bindings/c` builds a C library and its header, and `bindings/python` a Python extension; each runs the library's `count` on DIMACS text, a file or clauses held as data, and returns the mode, the count and the run's record.
- Build for WebAssembly through Emscripten, where a memory limit is refused, and count in the browser: a page under `bindings/wasm` runs the library in a worker on a pasted or opened CNF, and draws the compile of a formula of a few variables step by step, with the probability of each variable under evidence pinned on the page.
- Publish an evaluation beside the code: the exact counters entered in the 2026 model counting competition and four d-DNNF compilers, run with tididi-bonsai on the competition's instances of 2025 and 2026 on one machine under the same limits. `docs/evaluation.md` states the method and an interactive page beside the browser page shows every run.
