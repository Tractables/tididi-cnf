# Agent instructions

`CLAUDE.md` is the one instruction file in this repository; `AGENTS.md` is a
symlink to it, so every agent reads the same rules.

This is the public source repository of tididi-cnf: the `tididi` binary,
the `tididi_cnf` library, their tests and documentation, and the C, Python
and WebAssembly bindings under `bindings/`. Every tracked file and every commit
is public.

**[CONTRIBUTING.md](.github/CONTRIBUTING.md) binds in full**: the checks, the code and
test rules, and the doc and commit conventions. On top of it:

- Run `bash .github/scripts/verify.sh` before reporting a change done.
- Cargo manages the diagram library and vitri. Keep dependency updates in
  `Cargo.toml` and the committed lockfiles; no recursive clone is needed.
- For a bug fix, confirm the regression test fails on the parent commit
  before writing the fix.
- Set a timeout on any run over an unfamiliar formula.
- User-facing behaviour is documented in `docs/` and `--help`; change the
  documentation in the same commit as the behaviour.
