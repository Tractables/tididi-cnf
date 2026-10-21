# Contributing

Build prerequisites: [`docs/building.md`](docs/building.md).

## Checks

Before opening a pull request, run:

```sh
bash scripts/verify.sh
```

- The script builds the crate in the debug and release profiles
  `--locked`, against the committed `Cargo.lock`, runs Clippy and rustdoc
  with warnings denied, runs the crate's tests and doctests, and runs
  `scripts/check_docs.py` after its self-test: every worked example's
  program, and every command the README and the user guide follow with its
  output, against that output. Each step has a 30-minute limit, and the
  script stops at the first failure with its exit status.
- The release build has debug assertions off, which catches an item gated on
  them that ungated code still names.
- The tests run one at a time, in the script and in the thorough tier,
  because they call vitri in-process and vitri is not known to be safe to
  enter from two threads at once. The thorough tier adds the long tests:
  `cargo test -- --include-ignored --test-threads=1`.

CI runs this same script. It also checks that every file naming the release
version names the same one (`.github/scripts/check-versions.py` lists them),
and compiles the crate with the oldest supported Rust, the `rust-version`
in `Cargo.toml`.

## Worked examples

Each worked example, a page in [`docs/examples/`](docs/examples/), has a
runnable program, `examples/<name>.rs`, and every excerpt on the page is taken
from it: change the program and the page together, and mark each excerpt with
the `tested-example` class. A printing excerpt is followed by `Output:` and a
`text` block. `tests/example_docs.rs` checks the excerpts against the program;
`scripts/check_docs.py` runs each program and checks that those blocks occur
in its output, and runs the `sh` blocks of the README and `docs/*.md` against
the `Output:` or `Stderr:` block that follows them. Build with
`cargo build --release` first.

## The protocol test

`tests/protocol.rs` runs the binary on a fixed set of instances that covers
every counting task and both routes of the model count, with and without
`--vtree`, and pins each run's exit status, stdout and stderr, the statistics
record included, against the transcripts in `tests/data/protocol/`. A change
that moves a count, the path a run takes or a tally fails it. After an
intended change, regenerate the transcripts and read the diff before
committing them with the change:

```sh
UPDATE_PROTOCOL=1 cargo test --test protocol
```

## Code

Simplicity first: the simplest code that does the job is the goal, and every
rule below is a form of it. Review a change against this list before it lands.

**Design**

- Remove before you add. A new type, module, option or layer earns its place
  by deleting more code or concepts than it adds. No compatibility shims, no
  aliases, no wrapper that only forwards a call.
- One source of truth. One implementation per operation, one place that reads
  each setting or environment variable, one list for each vocabulary (modes,
  preprocessing levels, output formats) that drives parsing, `--help` and
  error messages. Vary behaviour with a parameter on the shared path, never
  with a copy that has to be kept in sync. A rule written in two places (a
  deadline test, a divisor, a default) becomes one helper.
- State is explicit. A value one function hands to another goes through a
  parameter, a return value or a struct the caller owns; a thread-local or a
  global is not a way to pass it. A function returns what it measured (a
  count, a peak size, a time), and the one caller that owns the run record
  writes it.
- Invalid states are unrepresentable. Options that only make sense together
  are one enum variant, not independent flags whose combinations can be inert.
- One concern per module, named for what it does, not for the library it
  wraps. Dependencies between modules point one way. Items are private unless
  another module uses them, and public only when a library caller needs them.
- Diagram representation and operations belong in
  [tididi](https://github.com/Tractables/tididi); CNF parsing, preprocessing and
  vtree construction belong in vitri. A copy of one of their rules here is a
  defect: change them there first, then update the Cargo dependency and lockfiles
  here.

**Behaviour**

- A setting the run would ignore is an error, never silently dropped. The
  error names the setting and the task or mode it needs, and the check is on
  the setting's presence, not its resolved value.
- Invalid input returns an `Error` that names it. A panic means a broken
  internal invariant, never a malformed formula, vtree or option.
- The compile is single-threaded. Callers get parallelism by running instances
  side by side; do not add threads.
- A feature lands switched on, with the evidence that earns it; there are no
  default-off features. An environment variable is a diagnostic or an
  emergency switch, not a way to ship something off.
- Instrumentation goes with its trigger: a counter that nothing can switch on,
  or nothing reads, is dead code, and so is a trace kept from an investigation
  that has ended.
- The compile loop is the hot path. A change there adds no per-step
  allocation, hashing or clock read without evidence that it pays.
- The release profile keeps `panic = "unwind"`, Rust's default, so neither
  `Cargo.toml` nor `RUSTFLAGS` names a panic strategy: memory recovery catches
  the unwind of a failed allocation, and `abort` would disable it without an
  error.

**Comments**

- A comment says what the code does and why. No history ("used to", "was
  replaced by"), no benchmark numbers, no names of retired mechanisms, no line
  numbers. An invariant is stated once, where it is enforced, and referenced
  elsewhere.
- Do not run rustfmt over the crate; it is not rustfmt-formatted. New code
  follows the formatting around it.

## Tests

- A module's unit tests are a child module in a file beside it,
  `<dir>/tests/<stem>.rs` (`<dir>/tests/mod.rs` for a `mod.rs`), declared as
  `#[cfg(test)] #[path = "tests/<stem>.rs"] mod tests;`. A production file
  holds no other test-only item; the helpers unit tests share are in
  `src/tests/support.rs`. Integration tests are in `tests/`, with the shared
  CNF patterns, vtree shapes and brute-force oracle in `tests/common/`, and
  every input file a test or a doctest reads in `tests/data/`.
  `tests/source_layout.rs` enforces this layout, along with the comment rules:
  no all-caps emphasis, backticked item names, and citations of files that
  exist.
- A bug fix comes with a regression test that fails on the parent commit, in
  the same commit.
- Fixed seeds. A test that depends on the wall clock arranges its own cut, by
  waiting a deadline out or stopping on a step count, rather than racing a
  sleep against a deadline.
- Every test in the default tier does bounded work; a long one is
  `#[ignore]`d. Tests build at `opt-level = 1`, since the compile-heavy ones
  run far slower unoptimized. `--profile release-checked` runs the debug
  assertions at near-release speed when a bug only shows at scale.

## Docs and commits

- Docs describe current behaviour: what exists, what to watch out for, what is
  guaranteed. No development history or numbers that go stale (test counts,
  runtimes, sizes). Measured results appear in one place, the
  [evaluation](docs/evaluation.md) and the README section built from the same
  data, which name the release, machine and limits they measured and are
  rebuilt for each release.
- [`docs/architecture.md`](docs/architecture.md) maps the source, states the
  invariants and links the design pages. A new public module gets a row in
  its module table, which `tests/source_layout.rs` checks; a change to a
  mechanism updates its page in the same commit.
- Commit subjects are imperative and describe the behaviour changed, e.g.
  "Refuse a setting the run would ignore". The body says why, in plain prose.
- Commit messages must be suitable for publication: no tool footers, session
  links or machine-specific details.

## Releasing

Set the new version in `Cargo.toml`, in each binding's `Cargo.toml` under
`bindings/` and in `CITATION.cff`, then run `cargo update --workspace` in the
repository root and in each binding, so that every `Cargo.lock` follows.
Rename the `## Unreleased` heading in [`CHANGELOG.md`](CHANGELOG.md) to the
version, and move the worked examples' `blob/v<version>/` program links to the
new tag. CI refuses a file that still names the old version.

Then publish a GitHub release with the tag `v<version>`: the release workflow
checks the tag against those files, builds and runs the Linux binary, and
attaches it to the release with the licence texts it needs. Last, run the Docs
workflow with `publish` ticked, so that the documentation and the browser page
on GitHub Pages follow the release.
