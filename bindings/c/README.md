# tididi-cnf from C and C++

A C ABI over [tididi-cnf](https://github.com/Tractables/tididi-cnf), a
knowledge compiler and model counter for CNF: a CNF goes in, as DIMACS text, as
clauses of DIMACS integers or as a file, and its count comes back, with the
mode the run resolved to, whether the formula has a model, and the run's record
as JSON. Each call is the run the `tididi` binary makes on the same formula and
mode, without the answer it prints on stdout; its progress still goes to
stderr.

This is a separate crate with its own `Cargo.toml`. It builds the library in
the repository above it, and `tididi_cnf_version()` reports the version of
that library.

## Building

```sh
cd bindings/c
make
```

This runs `cargo` for both libraries and for the `tididi` binary the tests
compare against. The first build compiles vitri's C++ stack;
[`docs/building.md`](../../docs/building.md) lists the prerequisites.
The libraries land in `bindings/c/target/release`: `libtididi_cnf_c.so`
and `libtididi_cnf_c.a`. The header is `bindings/c/include/tididi_cnf.h`.

The Makefile and the CI workflow cover Linux x86_64.

## Linking

Against the shared library, nothing else is needed. Link it by name, so the
program records `libtididi_cnf_c.so` rather than the path it was built
from, and tell the loader where to find it:

```sh
cc -Ibindings/c/include prog.c -Lbindings/c/target/release -ltididi_cnf_c \
   -Wl,-rpath,"$PWD/bindings/c/target/release" -o prog
```

A static link has to name the system libraries the archive uses, which is the
`Libs.private` line of `tididi-cnf-c.pc.in`. GMP and MPFR are always linked
dynamically, whichever library you link.

```sh
cc -Ibindings/c/include prog.c bindings/c/target/release/libtididi_cnf_c.a \
   $(sed -n 's/^Libs.private: //p' bindings/c/tididi-cnf-c.pc.in) -o prog
```

That file is a pkg-config template; fill in its two placeholders when
installing, taking the version from `bindings/c/Cargo.toml`:

```sh
version=$(sed -n 's/^version = "\(.*\)"/\1/p' bindings/c/Cargo.toml | head -1)
sed -e 's|@PREFIX@|/usr/local|' -e "s|@VERSION@|$version|" \
    bindings/c/tididi-cnf-c.pc.in > /usr/local/lib/pkgconfig/tididi-cnf-c.pc
```

The static archive carries vitri's vendored CaDiCaL, CryptoMiniSat and Arjun
with their symbols visible. A program that links its own copy of any of them
should use the shared library.

## Using the API

```c
tididi_cnf_answer *answer = NULL;
if (tididi_cnf_count_file("formula.cnf", "mc", 60000, 0, &answer) == TIDIDI_CNF_OK)
  printf("%s\n", tididi_cnf_answer_count(answer, NULL));
else
  fprintf(stderr, "%s\n", tididi_cnf_answer_error_message(answer, NULL));
tididi_cnf_answer_free(answer);
```

A program that builds its formula passes the clauses as one array of DIMACS
literals, each clause closed by `0` as a SAT solver's API takes them, with the
`c p show` variables and the `c p weight` weights a projected or weighted count
reads. A weight is text, such as `0.3` or `3/10`, so it stays exact:

```c
static const int32_t literals[] = {1, 2, 0, -1, 3, 0, -2, -3, 0};
static const uint32_t show[] = {1, 2};
const tididi_cnf_weight weights[] = {{1, "0.3"}, {-1, "0.7"}, {2, "0.4"}};
tididi_cnf_count_clauses(3, literals, 9, show, 2, weights, 3, "pwmc", 60000, 0, &answer);
```

`tididi_cnf_answer_count` spells the count exactly, and
`tididi_cnf_answer_count_f64` gives it as the nearest `double`.
`include/tididi_cnf.h` documents every function, including who owns each
buffer and how calls from several threads behave. `examples/count.c` counts a
file and prints the answer, `examples/count.cpp` does the same from C++ with
the answer held by a `std::unique_ptr`, and `examples/clauses.c` counts the
formula above.

What a C program should know about a run:

- **It belongs to the process.** A nonzero `mem_limit_mb` lowers the process's
  address-space limit (`RLIMIT_AS`) while the run lasts, as `--mem-limit-mb`
  does, and the run prints its progress to stderr. The library counts with
  jemalloc as its Rust allocator, beside the program's own `malloc`.
- **One call at a time.** A call made while another thread's call runs waits
  for it.
- **The time limit is checked as the run goes**, so a step in progress can
  finish after it; [`docs/limits.md`](../../docs/limits.md) says what each
  limit does. To stop a run at a hard limit, run it in a process that can be
  killed, such as the `tididi` binary.

## Tests

```sh
make check            # the examples and tests/conformance.c, against each library
make link-libs-check  # Libs.private covers what rustc reports for a static link
```

`tests/conformance.c` compares the counts of the instances
[`bindings/parity.txt`](../parity.txt) lists, read from the file, from its text
and as clauses, with what the `tididi` binary prints for them, and checks the
error codes and the pointer contracts.
`cargo test --release --locked` runs the crate's own tests.

## Regenerating the header

`include/tididi_cnf.h` is generated from `src/lib.rs` by
[cbindgen](https://github.com/mozilla/cbindgen) and committed. CI regenerates
it and fails if the committed copy differs, so change `src/lib.rs` and then:

```sh
cargo install cbindgen --version 0.29.0 --locked
make header
```
