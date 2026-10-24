<p align="center">
  <img src="https://raw.githubusercontent.com/Tractables/tididi-cnf/main/docs/logo.svg" alt="tididi-cnf" width="460">
</p>

<p align="center">
  <a href="https://github.com/Tractables/tididi-cnf/actions/workflows/c.yml"><img
     src="https://github.com/Tractables/tididi-cnf/actions/workflows/c.yml/badge.svg" alt="C"></a>
  <a href="https://tractables.github.io/tididi-cnf/"><img
     src="https://img.shields.io/badge/run-in%20the%20browser-blue" alt="Run in the browser"></a>
  <a href="https://arxiv.org/abs/2604.05537"><img
     src="https://img.shields.io/badge/arXiv-2604.05537-b31b1b.svg" alt="arXiv"></a>
</p>

# tididi-cnf for C and C++

**A knowledge compiler and model counter for CNF, in C and C++.**

tididi-cnf compiles a CNF formula into a **Tree Decision Diagram (TDD)** and
counts its models exactly: the model count, the weighted count and their
projections onto a set of variables. On the 1,400 entries of the 2020–2026
Model Counting Competitions it compiles more formulas than any BDD, SDD or
logic-circuit compiler we compared, and counts more than every exact counter
of the 2026 competition but one
([evaluation](https://github.com/Tractables/tididi-cnf#how-well-does-it-work)).
This library exposes the same Rust implementation as the `tididi` command line
through a C interface: a formula goes in as DIMACS text, as clauses of DIMACS
integers or as a file, and its count comes back, with the mode the run
resolved to, whether the formula has a model, and the run's record as JSON.

## Install

Each [release](https://github.com/Tractables/tididi-cnf/releases/latest) has a
prebuilt package for x86-64 Linux: the header, the shared and static libraries,
and a pkg-config file. Unpack it anywhere; it needs no Rust toolchain. The
libraries load the system's GMP (`libgmp10` and `libgmpxx4ldbl` on Debian and
Ubuntu) and need glibc 2.35 or later (Ubuntu 22.04, Debian 12 and newer).

```sh
tar xzf tididi-cnf-c-v0.1.1-x86_64-unknown-linux-gnu.tar.gz
mv tididi-cnf-c-v0.1.1-x86_64-unknown-linux-gnu tididi-cnf-c
export PKG_CONFIG_PATH=$PWD/tididi-cnf-c/lib/pkgconfig
```

To build and install the same package from a checkout instead, with the
prerequisites [`docs/building.md`](../../docs/building.md) lists:

```sh
cd bindings/c
make install PREFIX=/your/install/prefix
```

## Getting started

This example counts the models of `(x1 ∨ x2) ∧ (¬x1 ∨ x3) ∧ (¬x2 ∨ ¬x3)` over
`x1` and `x2`, weighted by the probability that each is true, 0.3 for `x1` and
0.4 for `x2`: the probability that some value of `x3` completes them to a
model. The clauses are one array of DIMACS literals, each clause closed by `0`
as a SAT solver's API takes them, and a weight is text, so it stays exact:

```c
static const int32_t literals[] = {1, 2, 0, -1, 3, 0, -2, -3, 0};
static const uint32_t show[] = {1, 2};
const tididi_cnf_weight weights[] = {{1, "0.3"}, {-1, "0.7"}, {2, "0.4"}};
tididi_cnf_answer *answer = NULL;
if (tididi_cnf_count_clauses(3, literals, 9, show, 2, weights, 3, "pwmc", 60000, 0,
                             &answer) == TIDIDI_CNF_OK)
  printf("count: %s\n", tididi_cnf_answer_count(answer, NULL));
else
  fprintf(stderr, "%s\n", tididi_cnf_answer_error_message(answer, NULL));
tididi_cnf_answer_free(answer);
```

Output:

```text
count: 23/50
```

`tididi_cnf_count` takes DIMACS text and `tididi_cnf_count_file` a file,
whose header can name the task, or the directory of a vitri bundle.
`tididi_cnf_answer_count` spells the count exactly, and
`tididi_cnf_answer_count_f64` gives it as the nearest `double`.
`include/tididi_cnf.h` documents every function, including who owns each
buffer and how calls from several threads behave. `examples/clauses.c` is the
program above, `examples/count.c` counts a file and prints the answer, and
`examples/count.cpp` does the same from C++ with the answer held by a
`std::unique_ptr`.

A run belongs to the process:

- **A nonzero `mem_limit_mb` lowers the process's address-space limit**
  (`RLIMIT_AS`) while the run lasts, as `--mem-limit-mb` does, and the run
  prints its progress to stderr. The library counts with jemalloc as its Rust
  allocator, beside the program's own `malloc`.
- **One call at a time.** A call made while another thread's call runs waits
  for it.
- **The time limit is checked as the run goes**, so a step in progress can
  finish after it; [`docs/limits.md`](../../docs/limits.md) says what each
  limit does. To stop a run at a hard limit, run it in a process that can be
  killed, such as the `tididi` binary.

## Linking

Link the shared library by name, so the program records `libtididi_cnf_c.so`
rather than a path, and tell the loader where to find it:

```sh
cc prog.c $(pkg-config --cflags --libs tididi-cnf-c) -Wl,-rpath,"$PWD/tididi-cnf-c/lib" -o prog
```

A static link names the archive and the system libraries it uses, the
`Libs.private` line of the pkg-config file. GMP and MPFR are always linked
dynamically, whichever library you link.

```sh
cc prog.c $(pkg-config --cflags tididi-cnf-c) tididi-cnf-c/lib/libtididi_cnf_c.a \
   $(pkg-config --static --libs-only-l tididi-cnf-c | sed 's/-ltididi_cnf_c//') -o prog
```

The static archive carries vitri's vendored CaDiCaL, CryptoMiniSat and Arjun
with their symbols visible. A program that links its own copy of any of them
should use the shared library.

## Develop

`make` builds both libraries into `target/release`, and the `tididi` binary
the tests compare against; the first build compiles vitri's C++ stack. Run the
examples and the tests:

```sh
make check            # the examples and tests/conformance.c, against each library
make link-libs-check  # Libs.private covers what rustc reports for a static link
make install PREFIX=/tmp/tididi-cnf-c installcheck  # an example against the installed package
```

`tests/conformance.c` compares the counts of the instances
[`bindings/parity.txt`](../parity.txt) lists, read from the file, from its text
and as clauses, with what the `tididi` binary prints for them, and checks the
error codes and the pointer contracts.
`cargo test --release --locked` runs the crate's own tests.

`include/tididi_cnf.h` is generated from `src/lib.rs` by
[cbindgen](https://github.com/mozilla/cbindgen) and committed. CI regenerates
it and fails if the committed copy differs, so change `src/lib.rs` and then:

```sh
cargo install cbindgen --version 0.29.0 --locked
make header
```

## Citing

Please cite [*A Canonical Generalization of OBDD*](https://arxiv.org/abs/2604.05537),
which introduced TDDs.

## Acknowledgements

This material is based upon work supported by the Defense Advanced Research
Projects Agency (DARPA) under Agreement No. HR0011269E224.
