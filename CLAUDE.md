# Agent instructions

`CLAUDE.md` is the one instruction file in this repository; `AGENTS.md` is a
symlink to it, so every agent reads the same rules.

This is the public source repository of tididi-bonsai: the `tididi` binary,
the `tididi_bonsai` library, their tests and documentation, and the C, Python
and WebAssembly bindings under `bindings/`. Every tracked file and every commit
is public.

**[CONTRIBUTING.md](CONTRIBUTING.md) binds in full**: the checks, the code and
test rules, and the doc and commit conventions. On top of it:

- Run `bash scripts/verify.sh` before reporting a change done.
- The diagram library (`tididi/`) and vitri (`vendor/vitri/`) are submodules;
  run `git submodule update --init --recursive` before the first build. A
  change to either goes to its own repository first; the pointer here then
  moves forward in a commit of its own.
- For a bug fix, confirm the regression test fails on the parent commit
  before writing the fix.
- A change that moves a count, the path a run takes or a tally fails the
  protocol test; regenerate its transcripts and read the diff, as
  CONTRIBUTING.md says.
- Set a timeout on any run over an unfamiliar formula.
- User-facing behaviour is documented in `docs/` and `--help`; change the
  documentation in the same commit as the behaviour.
