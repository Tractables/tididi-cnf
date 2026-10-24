<p align="center">
  <img src="https://raw.githubusercontent.com/Tractables/tididi-cnf/main/docs/logo.svg" alt="tididi-cnf" width="460">
</p>

<p align="center">
  <a href="https://pypi.org/project/tididi-cnf/"><img
     src="https://img.shields.io/pypi/v/tididi-cnf.svg" alt="PyPI"></a>
  <a href="https://github.com/Tractables/tididi-cnf/actions/workflows/python.yml"><img
     src="https://github.com/Tractables/tididi-cnf/actions/workflows/python.yml/badge.svg" alt="Python"></a>
  <a href="https://tractables.github.io/tididi-cnf/"><img
     src="https://img.shields.io/badge/run-in%20the%20browser-blue" alt="Run in the browser"></a>
  <a href="https://arxiv.org/abs/2604.05537"><img
     src="https://img.shields.io/badge/arXiv-2604.05537-b31b1b.svg" alt="arXiv"></a>
</p>

# tididi-cnf for Python

**A knowledge compiler and model counter for CNF, in Python.**

tididi-cnf compiles a CNF formula into a **Tree Decision Diagram (TDD)** and
counts its models exactly: the model count, the weighted count and their
projections onto a set of variables. On the 1,400 entries of the 2020–2026
Model Counting Competitions it compiles more formulas than any BDD, SDD or
logic-circuit compiler we compared, and counts more than every exact counter
of the 2026 competition but one
([evaluation](https://github.com/Tractables/tididi-cnf#how-well-does-it-work)).
This package runs the same Rust implementation as the `tididi` command line:
each call compiles or counts a formula as the binary does, and returns the
answer instead of printing it.

## Install

```sh
pip install tididi-cnf
```

The wheel covers Python 3.10 and later on x86-64 Linux with glibc 2.34 or later
(Ubuntu 22.04, Debian 12, RHEL 9 and newer), and carries the GMP library it
needs. Elsewhere on Linux, build it from a checkout, as [Develop](#develop)
says.

## Getting started

This example counts the models of a three-clause formula, then the probability
that the formula holds when `x1` is true with probability 0.3 and `x2` with
probability 0.4, whatever `x3` does:

```python
import tididi_cnf

# (x1 ∨ x2) ∧ (¬x1 ∨ x3) ∧ (¬x2 ∨ ¬x3)
clauses = [[1, 2], [-1, 3], [-2, -3]]
print("Models:", tididi_cnf.count(clauses).count)

answer = tididi_cnf.count(clauses, mode="pwmc", show=[1, 2], weights={1: "0.3", 2: "0.4"})
print("Probability:", answer.count)
```

Output:

```text
Models: 2
Probability: 23/50
```

The second call is a projected weighted count: it adds up the weights of the
assignments to `x1` and `x2` that some value of `x3` completes to a model.
Counts are exact: an `int`, or a `fractions.Fraction` for weighted projected
counts. A weighted count without projection is a `float`.

`tididi_cnf.count` takes DIMACS text as `str` or `bytes`, clauses of nonzero
`int` literals, or an object with `clauses` and `nv` such as a PySAT `CNF`.
`show` and `weights` stand for a DIMACS file's `c p show` and `c p weight`
lines. A weight is an `int`, a `Fraction`, a decimal string read exactly, or a
`float` read as its shortest decimal; a lone weight for `x` leaves `1 - w` for
`¬x`. `tididi_cnf.count_file` reads a DIMACS file, whose header can name the
task, or the directory of a vitri bundle:

```python
answer = tididi_cnf.count_file("formula.cnf", time_limit_ms=60_000)

answer.mode         # "mc", "wmc", "pmc", "pwmc", or "compile" without a task line
answer.satisfiable  # whether the formula has a model
answer.count        # the count
answer.stats        # the run's record, the object `tididi --stats` writes
```

Both calls take `mode`, which must agree with the formula's `c t` and
`c p show` lines and names the task for a formula without them, and the
limits `time_limit_ms` and `mem_limit_mb`. A run that fails raises a subclass
of `tididi_cnf.Error`; `help(tididi_cnf.count)` lists them. The user guide's
[input](https://github.com/Tractables/tididi-cnf/blob/main/docs/input.md),
[tasks](https://github.com/Tractables/tididi-cnf/blob/main/docs/tasks.md) and
[limits](https://github.com/Tractables/tididi-cnf/blob/main/docs/limits.md)
pages describe the formula, the counts and the limits, and
[`examples/count.py`](https://github.com/Tractables/tididi-cnf/blob/main/bindings/python/examples/count.py)
counts a file from the command line.

A run belongs to the process:

- **A nonzero `mem_limit_mb` lowers the interpreter's address-space limit**
  (`RLIMIT_AS`) while the run lasts, and the run prints its progress to the
  process's stderr, not to `sys.stderr`.
- **The interpreter lock is released while it runs**, and calls from several
  threads run one at a time.
- **The time limit is checked as the run goes**, so a step in progress can
  finish after it. For a hard wall-clock limit, make the call in a separate
  process and stop that process at the limit.

## Develop

Building the package needs the Rust and C++ toolchains and the system packages
that [`docs/building.md`](https://github.com/Tractables/tididi-cnf/blob/main/docs/building.md)
lists. From the repository root, build it in place, then build the command
line and run the tests. They also compare the version and the counts of the
instances
[`bindings/parity.txt`](https://github.com/Tractables/tididi-cnf/blob/main/bindings/parity.txt)
lists with the binary's:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install 'maturin>=1.9.4,<2' pytest
maturin develop --release --locked --manifest-path bindings/python/Cargo.toml
cargo build --release --locked --bin tididi
python -m pytest bindings/python/tests --tididi-cli target/release/tididi
```

## Citing

Please cite [*A Canonical Generalization of OBDD*](https://arxiv.org/abs/2604.05537),
which introduced TDDs.

## Acknowledgements

This material is based upon work supported by the Defense Advanced Research
Projects Agency (DARPA) under Agreement No. HR0011269E224.
