# tididi-cnf for Python

Python bindings for [tididi-cnf](https://github.com/Tractables/tididi-cnf),
a knowledge compiler and model counter for CNF, built with
[PyO3](https://pyo3.rs) and [maturin](https://www.maturin.rs). tididi-cnf
compiles a formula into a Tree Decision Diagram and counts its models exactly:
the model count, the weighted count and their projections onto a set of
variables. Each call compiles or counts the formula as the `tididi` binary
does on the same input and mode, and returns the answer instead of printing
it; its progress still goes to stderr.

## Use

```python
import tididi_cnf

answer = tididi_cnf.count_file("formula.cnf", time_limit_ms=60_000)

answer.mode         # "mc", "wmc", "pmc", "pwmc", or "compile" without a task line
answer.satisfiable  # whether the formula has a model
answer.count        # an int, a fractions.Fraction for pwmc, a float for wmc
answer.stats        # the run's record, the object `tididi --stats` writes
```

`tididi_cnf.count(formula)` counts a formula held in the program: DIMACS
text as `str` or `bytes`, or clauses of nonzero `int` literals, as lists or
any other iterables, or an object with `clauses` and `nv` such as a PySAT
`CNF`. Clauses take the keywords `num_vars`, `show` and `weights` for what a
DIMACS file declares in its `p cnf`, `c p show` and `c p weight` lines. A
weight is an `int`, a `Fraction`, a decimal string read exactly, or a `float`
read as its shortest decimal:

```python
# (x1 or x2) and (not x1 or x3) and (not x2 or not x3), counted over x1 and
# x2, with x1 weighing 0.3 true and 0.7 false, and x2 0.4 true, so 0.6 false
# as the competition's format reads a lone weight.
answer = tididi_cnf.count(
    [[1, 2], [-1, 3], [-2, -3]], mode="pwmc", show=[1, 2], weights={1: "0.3", -1: "0.7", 2: 0.4}
)
answer.count  # Fraction(23, 50)
```

Both calls take `mode`, which must agree with the instance's `c t` and
`c p show` lines and names the task for a formula without them, and the
limits `time_limit_ms` and `mem_limit_mb`. A run that fails raises a subclass
of `tididi_cnf.Error`; `help(tididi_cnf.count)` and
`help(tididi_cnf.count_file)` list them. The user guide's
[input](https://github.com/Tractables/tididi-cnf/blob/main/docs/input.md),
[tasks](https://github.com/Tractables/tididi-cnf/blob/main/docs/tasks.md) and
[limits](https://github.com/Tractables/tididi-cnf/blob/main/docs/limits.md)
pages describe the formula, the counts and the limits.

What a Python program should know about a run:

- **It belongs to the process.** A nonzero `mem_limit_mb` lowers the
  interpreter's address-space limit (`RLIMIT_AS`) while the run lasts, and the
  run prints its progress to the process's stderr, not to `sys.stderr`.
- **The interpreter lock is released while it runs**, and calls from several
  threads run one at a time.
- **The time limit is checked as the run goes**, so a step in progress can
  finish after it. For a hard wall-clock limit, make the call in a separate
  process and stop that process at the limit.

[`examples/count.py`](https://github.com/Tractables/tididi-cnf/blob/main/bindings/python/examples/count.py)
counts a file and prints the answer.

## Building from source

The extension compiles the library in this repository, and vitri's C++ stack
with it, so it needs the toolchain and system packages that
[`docs/building.md`](https://github.com/Tractables/tididi-cnf/blob/main/docs/building.md)
lists. From the repository root, create a virtual environment and install
the extension with maturin:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install maturin
maturin develop --release --locked --manifest-path bindings/python/Cargo.toml
```

The extension links the system GMP dynamically. No wheel is published yet.

To build a wheel instead, run
`maturin build --release --locked --manifest-path bindings/python/Cargo.toml --compatibility linux --auditwheel skip`.
That wheel links the system libraries and is for a compatible Linux machine.

Run the tests in the same environment, from the repository root:

```sh
python -m pip install pytest
cargo build --release --locked --bin tididi
python -m pytest bindings/python/tests --tididi-cli target/release/tididi
```

The tests also compare the version and the counts of the instances
[`bindings/parity.txt`](https://github.com/Tractables/tididi-cnf/blob/main/bindings/parity.txt)
lists with the binary's.
