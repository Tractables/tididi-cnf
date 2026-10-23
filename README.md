<p align="center">
  <img src="docs/logo.svg" alt="tididi-cnf" width="460">
</p>

<p align="center">
  <a href="https://tractables.github.io/tididi-cnf/"><img
     src="https://img.shields.io/badge/run-in%20the%20browser-blue" alt="Run in the browser"></a>
  <a href="https://github.com/Tractables/tididi-cnf/actions/workflows/rust.yml"><img
     src="https://github.com/Tractables/tididi-cnf/actions/workflows/rust.yml/badge.svg" alt="CI"></a>
  <a href="https://tractables.github.io/tididi-cnf/tididi_cnf/"><img
     src="https://img.shields.io/badge/docs-Rust-blue" alt="Rust guide"></a>
  <a href="https://arxiv.org/abs/2604.05537"><img
     src="https://img.shields.io/badge/arXiv-2604.05537-b31b1b.svg" alt="arXiv"></a>
  <a href="LICENSE"><img
     src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License: Apache-2.0"></a>
</p>

**A knowledge compiler and model counter for CNF.**

tididi-cnf compiles a DIMACS CNF formula into a **Tree Decision Diagram
(TDD)**, a canonical circuit that holds all of the formula's models. Compile
once, then save the circuit, combine it with new constraints and query it as
evidence or weights change. The same compiler is a model counter: it answers
model counting, weighted model counting and projected model counting, with or
without weights, in the format of the Model Counting Competition.

On all 1,400 entries of the 2020–2026 Model Counting Competitions, tididi-cnf
compiles more of them than any of the BDD, SDD and logic-circuit compilers we
compared, and as a model counter it counts more than every exact counter of
the 2026 competition except Ganak, and 66 entries only it counts.

tididi-cnf is built on the [tididi](https://github.com/Tractables/tididi)
diagram library and the [vitri](https://github.com/Tractables/vitri) CNF
toolkit. [Try it in your browser](https://tractables.github.io/tididi-cnf/), or
read [*A Canonical Generalization of OBDD*](https://arxiv.org/abs/2604.05537),
which introduced TDDs.

## How well does it work?

Every tool runs on all 1,400 track-1 entries of the 2020–2026 Model Counting
Competitions, 1,153 distinct formulas, with two minutes, one CPU and 60 GiB
per instance. The [evaluation](docs/evaluation.md) has the full tables and the
method, and the [interactive plots](https://tractables.github.io/tididi-cnf/evaluation/)
show every run.

### Compiling, against decision-diagram packages

tididi-cnf compiles **712** of the 1,400 entries, **521** of which no BDD
or SDD package compiles. The best BDD package, BuDDy, compiles 157, and SDD
compiles 128 with dynamic vtree search. Every
BDD package gets a FORCE variable order, CUDD also with dynamic sifting, and
SDD starts from a FORCE vtree, all prepared outside the timer
([method](docs/evaluation.md#bottom-up-decision-diagram-compilation)).

<p align="center">
  <img src="docs/evaluation/cactus-2020-2026-dd-compile-120s.svg" width="760"
       alt="Cactus plot of the 1,400 MCC 2020–2026 entries compiled against time per instance, up to two minutes, by tididi-cnf, five BDD packages including CUDD with and without sifting, and SDD in fixed and dynamic modes">
</p>

### Compiling, against logic-circuit compilers

tididi-cnf compiles **712** entries, 134 more than Panini (578) and well ahead
of D4 (487), GPMC (482) and c2d (387). **160** of them no other compiler
finishes; Panini has 36 such entries.

<p align="center">
  <img src="docs/evaluation/cactus-2020-2026-logic-compile-120s.svg" width="760"
       alt="All 1,400 MCC 2020–2026 entries compiled within two minutes by tididi-cnf, Panini, D4 as a compiler, GPMC as a compiler and c2d">
</p>

### Counting, against the exact counters of the 2026 Model Counting Competition

tididi-cnf counts **770** of the 1,400 entries, second to Ganak (792) and
ahead of D4 (629), GPMC (577) and Cara (517). It is the only counter to count
**66** of them, more than Ganak's 42.

<p align="center">
  <img src="docs/evaluation/cactus-2020-2026-120s.svg" width="760"
       alt="Cactus plot of all 1,400 MCC 2020–2026 entries solved against time per instance, up to two minutes, for tididi-cnf, Ganak, D4 as a counter, GPMC as a counter and Cara">
</p>

<details>
<summary>One hour per instance</summary>

With an hour each on the 100 private instances of 2026, tididi-cnf counts 61,
Ganak and D4 62, Cara 58 and GPMC 54. Two of them only tididi-cnf counts, one
only Cara, and none only Ganak or D4.

<p align="center">
  <img src="docs/evaluation/cactus-3600s.svg" width="760"
       alt="Cactus plot of instances solved against time per instance, up to an hour, on the 100 private instances of 2026, for tididi-cnf, Ganak, D4 as a counter, GPMC as a counter and Cara">
</p>

</details>

## Install

- **Command line:** download the Linux x86-64 binary from the
  [latest release](https://github.com/Tractables/tididi-cnf/releases/latest),
  or build it with Rust and a C++20 toolchain
  ([prerequisites](docs/building.md)):

  ```sh
  git clone https://github.com/Tractables/tididi-cnf
  cd tididi-cnf
  cargo build --release
  ```

- **Rust:** depend on the crate through Git:
  `tididi-cnf = { git = "https://github.com/Tractables/tididi-cnf" }`.
- **Python:** build the package from a checkout, as the
  [Python README](bindings/python/README.md) says.
- **C and C++:** build the shared and static libraries with `make`, as the
  [C README](bindings/c/README.md) says.
- **No install:** [run it in your browser](https://tractables.github.io/tididi-cnf/).

## Getting started

`tests/data/queens8.cnf` is the eight queens puzzle: 64 variables, one per
square, and clauses that put a queen in every row and keep any two from
sharing a column or a diagonal. Compile it and save the circuit:

```sh
./target/release/tididi --save-tdd queens.tdd tests/data/queens8.cnf
```

Output:

```text
model count: 92
tdd written to queens.tdd, vtree to queens.tdd.vtree: 788 nodes, 879 pairs
```

A formula whose header names a task is counted instead.
`tests/data/wireless6_projected.cnf` is a 6 × 6 grid of radio sites, each
active when either of its two hidden switches is on, and its `c t pmc` line
asks how many activation patterns keep neighbouring sites from transmitting
together, whatever the switches do:

```sh
./target/release/tididi tests/data/wireless6_projected.cnf
```

Output:

```text
projected model count: 5598861
```

[Getting started](docs/getting-started.md) works through compiling and each
kind of count on a small formula, and [tasks](docs/tasks.md) lists the
options.

## Library

A compiled circuit is a [tididi](https://github.com/Tractables/tididi)
diagram, so a program keeps querying it without compiling again. This one
counts the placements of eight queens with a queen in a corner:

```rust
use tididi_cnf::tididi::Literal;
use tididi_cnf::{count, Limits, Mode};

let answer = count("tests/data/queens8.cnf", Some(Mode::Compile), &Limits::default())?;
let board = answer.diagram().expect("a compile keeps its circuit");
println!("Placements: {}", board.model_count()?);
let mut counter = board.counter()?;
counter.observe([Literal::try_from(1)?])?;
println!("With a queen in a corner: {}", counter.model_count()?);
```

Output:

```text
Placements: 92
With a queen in a corner: 4
```

With a counting mode, such as `Mode::Mc` or `Mode::Pwmc`, the same call
returns the count, as the [Python](bindings/python/README.md) and
[C](bindings/c/README.md) bindings do.

The worked examples cover what the compiler and counter do:

- **Compile rules once and query them** as choices change: the
  [configuration example] counts a laptop shop's configurations as a customer
  chooses, and adds a rule to the compiled circuit.
- **Count with weights** to compute probabilities: the [reliability example]
  finds how likely two sites of a network stay connected.
- **Count projections**, assignments to some variables that the rest can
  complete: the [circuit example] counts the distinct outputs of a multiplier.
- **Choose a vtree**, the tree a TDD follows: the [vtree example] compiles one
  formula on several and compares the circuits.

The [command-line guide](docs/getting-started.md) covers the
[input format](docs/input.md), the [output](docs/output.md), the
[run record](docs/stats.md) and the [time and memory limits](docs/limits.md).
The [API reference] documents the library, and contributors can start with the
[architecture reference](docs/architecture.md),
the [algorithms](docs/algorithms.md) and the
[contributing guide](.github/CONTRIBUTING.md).

## Citing

Please cite [*A Canonical Generalization of OBDD*](https://arxiv.org/abs/2604.05537),
which introduced TDDs; [CITATION.cff](CITATION.cff) has the details.
tididi-cnf is licensed under the Apache License 2.0 ([LICENSE](LICENSE),
[NOTICE](NOTICE)).

## Acknowledgements

This material is based upon work supported by the Defense Advanced Research
Projects Agency (DARPA) under Agreement No. HR0011269E224.

[API reference]: https://tractables.github.io/tididi-cnf/tididi_cnf/
[configuration example]: https://tractables.github.io/tididi-cnf/tididi_cnf/guide/examples/catalog/index.html
[reliability example]: https://tractables.github.io/tididi-cnf/tididi_cnf/guide/examples/reliability/index.html
[circuit example]: https://tractables.github.io/tididi-cnf/tididi_cnf/guide/examples/circuits/index.html
[vtree example]: https://tractables.github.io/tididi-cnf/tididi_cnf/guide/examples/vtree_choice/index.html
