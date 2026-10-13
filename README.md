<p align="center">
  <img src="docs/logo.svg" alt="tididi bonsai" width="460">
</p>

<p align="center">
  <a href="https://github.com/Tractables/tididi-bonsai/actions/workflows/rust.yml"><img
     src="https://github.com/Tractables/tididi-bonsai/actions/workflows/rust.yml/badge.svg" alt="CI"></a>
  <a href="https://tractables.github.io/tididi-bonsai/tididi_bonsai/"><img
     src="https://img.shields.io/badge/docs-main-blue" alt="Documentation"></a>
  <a href="LICENSE"><img
     src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License: Apache-2.0"></a>
</p>

**Count the solutions of a CNF formula exactly, or compile it into a circuit you can query.**

tididi-bonsai reads a propositional formula in DIMACS CNF and answers how many
assignments satisfy it, optionally weighted and optionally counted over a
chosen subset of the variables. It gets there by compiling the formula into a
**Tree Decision Diagram** (TDD), a canonical circuit on which counting takes
one pass. Where an OBDD tests variables in one fixed order, a TDD follows a
*vtree*, a binary tree over the variables. TDDs are introduced in
[*A Canonical Generalization of OBDD*](https://arxiv.org/abs/2604.05537).

The compiler simplifies the formula and chooses the vtree with
[vitri](https://github.com/Tractables/vitri), and builds the diagram with the
[tididi](https://github.com/Tractables/tididi) library. It is a command-line
tool, `tididi`, and a Rust library, `tididi_bonsai`.

## What it answers

**Model counting.** How many assignments satisfy the formula: the valid
configurations of a product whose options constrain one another, say. The
[configuration example] counts a laptop shop's offer and then counts again as
a customer chooses.

**Weighted model counting.** The total weight of those assignments, each
weighing the product of its literals' weights. When the weights are
probabilities, the count is the probability that the formula holds. The
[reliability example] computes how likely a network with failing links still
connects two sites.

**Projected counting.** The same questions asked of some of the variables
only: each assignment to them counts once if the rest of the formula can be
completed. An encoding's auxiliary variables stay out of the count this way.
The [circuit example] counts the distinct products a multiplier outputs.

**Compilation.** The TDD itself, saved to a file that the tididi library reads
back and queries: counts under evidence, weighted evaluation, conjunction with
further constraints.

## Build

Linux only. Besides Rust, the build needs a C++20 compiler, CMake,
`pkg-config` and the GMP, MPFR and zlib development packages;
[docs/building.md](docs/building.md) lists them. The diagram library and
vitri are git submodules:

```sh
git clone --recurse-submodules https://github.com/Tractables/tididi-bonsai
cd tididi-bonsai
cargo build --release
```

The binary is `target/release/tididi`.

## Command line

```sh
./target/release/tididi tests/data/mcc_mc_sat.cnf
```

Output:

```text
model count: 22
```

The file is Example 1 of the model counting competition's input format: six
variables, four clauses, and a `c t mc` line that asks for the model count.
Progress goes to stderr; the answer is the one line on stdout.
[Getting started](docs/getting-started.md) works the count out by hand and
then asks the other questions of the same formula.

A run answers one task. The instance's header lines name it, as the
competition's format has them, and `--mode` can name it too; the two must
agree.

| task | header lines | flag | answer line |
|---|---|---|---|
| model count | `c t mc` | `--mode mc` | `model count: 22` |
| weighted model count | `c t wmc`, `c p weight` | `--mode wmc` | `weighted count: 3.2999999999999985e0` |
| projected model count | `c t pmc`, `c p show` | `--mode pmc` | `projected model count: 3` |
| projected weighted model count | `c t pwmc`, `c p show`, `c p weight` | `--mode pwmc` | `projected weighted count: 22/25` |
| compilation | none | `--mode compile` | `model count: 22` |

Every count is exact except the weighted model count, which runs in
double-precision arithmetic: 3.3 prints as `3.2999999999999985e0`.
`--save-tdd <file>` writes the compiled circuit, `--stats <path>` a JSON
record of the run, and `tididi --help` lists every flag.

Larger instances are in our collection
[Tractables/cnfs](https://github.com/Tractables/cnfs), which holds the
[model counting competition](https://mccompetition.org)'s instances by year
and track as `.cnf.xz`; decompress one before a run.
[Running many instances](docs/limits.md#many-instances) says how to count a
directory of them side by side.

## Library

This program counts the models of (x1 ∨ x2) ∧ (¬x1 ∨ x3) ∧ (¬x2 ∨ ¬x3),
given as DIMACS text:

```rust
use tididi_bonsai::{count, parse_dimacs, Limits, Mode};

let formula = parse_dimacs("p cnf 3 3\n1 2 0\n-1 3 0\n-2 -3 0\n".as_bytes())?;
let answer = count(formula, Some(Mode::Mc), &Limits::default())?;
println!("Models: {}", answer.count());
```

Output:

```text
Models: 2
```

[`count`] runs what the binary runs: preprocessing, vtree selection,
compilation and the fallbacks for an instance that runs out of time or memory.
Below it, [`compile_cnf`] compiles a formula on a vtree you choose; the
[vtree example] compares the circuits several vtrees give. A program that
builds its formula as data passes the clauses to [`formula_from_clauses`]
instead of writing DIMACS text. The crate is not on crates.io yet; depend on
it through git:

```toml
[dependencies]
tididi-bonsai = { git = "https://github.com/Tractables/tididi-bonsai" }
```

## Bindings

The same counts are available through [Python](bindings/python/README.md),
[C](bindings/c/README.md) and [WebAssembly](bindings/wasm/README.md), from
DIMACS text or from clauses held as data. The WebAssembly build runs
[in the browser](https://tractables.github.io/tididi-bonsai/).

## Documentation

- [Worked examples] and the [API reference], with the
  [architecture reference] for contributors.
- The command-line guide: [getting started](docs/getting-started.md), the
  [tasks](docs/tasks.md), the [input format](docs/input.md), the
  [output](docs/output.md), [time and memory limits](docs/limits.md), the
  [run record](docs/stats.md) and the [environment variables](docs/env.md).
- Vtree construction and preprocessing are documented with
  [vitri](https://github.com/Tractables/vitri), the diagrams with
  [tididi](https://github.com/Tractables/tididi).
- [Building](docs/building.md) and [contributing](CONTRIBUTING.md).

To browse the documentation locally, run
`DOCS_RS=1 cargo doc --no-deps -p tididi-bonsai --open`; it needs none of the
C++ toolchain ([building](docs/building.md#documentation-builds)).

## Citing

TDDs were introduced in the following paper; [`CITATION.cff`](CITATION.cff)
cites it for this software.

```bibtex
@article{capelli2026canonical,
  title   = {A Canonical Generalization of {OBDD}},
  author  = {Capelli, Florent and Choi, YooJung and Mengel, Stefan and
             Mu{\~n}oz, Mart{\'i}n and Van den Broeck, Guy},
  journal = {arXiv preprint arXiv:2604.05537},
  year    = {2026},
  doi     = {10.48550/arXiv.2604.05537}
}
```

## License

Apache License, Version 2.0 ([LICENSE](LICENSE)). [NOTICE](NOTICE) lists the
components the build includes and where their licences are, among them the
C++ solvers vitri vendors.

[`count`]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/fn.count.html
[`compile_cnf`]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/compile/fn.compile_cnf.html
[`formula_from_clauses`]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/fn.formula_from_clauses.html
[configuration example]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/guide/examples/catalog/index.html
[reliability example]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/guide/examples/reliability/index.html
[circuit example]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/guide/examples/circuits/index.html
[vtree example]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/guide/examples/vtree_choice/index.html
[Worked examples]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/guide/examples/index.html
[API reference]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/
[architecture reference]: https://tractables.github.io/tididi-bonsai/tididi_bonsai/guide/architecture/index.html
