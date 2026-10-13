#!/usr/bin/env python3
"""Run what the documentation shows and compare it with the documented output.

Usage: python3 scripts/check_docs.py [--self-test]

Two kinds of page are checked, from the repository root:

- The worked examples, `docs/examples/<name>.md`. Each has a program
  `examples/<name>.rs` and the other way round. The program runs with
  `cargo run --release --locked -p tididi-bonsai --example <name>` (the
  library has examples of the same names), and every `Output:` block of the
  page must occur in its stdout, as whole lines and in page order.
- The command-line guide, `README.md` and `docs/*.md`. A `sh` block followed by
  an `Output:` or a `Stderr:` block runs in a scratch directory that sees the
  repository's `tests/`, with `target/release` first on the `PATH`, so a
  command spelled `tididi` runs the release binary; its stdout, or its
  stderr, must equal the block. A text block with neither label is an
  illustration (one with timings, say) and is not checked. Build the binary
  first with `cargo build --release`.

Exits nonzero, naming the page and the block, when anything differs.
"""

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BLOCK = re.compile(r"^```([^\n]*)\n(.*?)^```[ \t]*$", re.M | re.S)
LABELS = {"\n\nOutput:\n\n": "stdout", "\n\nStderr:\n\n": "stderr"}


def output_blocks(markdown):
    """Each labelled text block: the stream it shows, the block just before it
    (its language and body), and its own body."""
    blocks = list(BLOCK.finditer(markdown))
    for before, block in zip(blocks, blocks[1:]):
        language, body = block.groups()
        stream = LABELS.get(markdown[before.end():block.start()])
        if language == "text" and stream:
            yield stream, before.group(1), before.group(2), body


def contains_in_order(stdout, expected):
    """Whether the expected blocks occur in stdout as whole lines, in order."""
    actual = "\n" + stdout
    cursor = 0
    for body in expected:
        position = actual.find("\n" + body, cursor)
        if position < 0:
            return body
        cursor = position + len(body)
    return None


def check_walkthroughs(failures):
    pages = {path.stem: path for path in (ROOT / "docs" / "examples").glob("*.md")}
    programs = {path.stem: path for path in (ROOT / "examples").glob("*.rs")}
    for name in sorted(set(pages) ^ set(programs)):
        failures.append(f"{name}: a worked example needs both docs/examples/{name}.md and examples/{name}.rs")
    for name in sorted(set(pages) & set(programs)):
        markdown = pages[name].read_text(encoding="utf-8")
        expected = [body for stream, _, _, body in output_blocks(markdown) if stream == "stdout"]
        if not expected:
            failures.append(f"docs/examples/{name}.md: no Output block")
            continue
        run = subprocess.run(
            ["cargo", "run", "--quiet", "--release", "--locked", "-p", "tididi-bonsai", "--example", name],
            cwd=ROOT, capture_output=True, text=True,
        )
        if run.returncode != 0:
            failures.append(f"examples/{name}.rs: exit status {run.returncode}\n{run.stderr[-2000:]}")
            continue
        missing = contains_in_order(run.stdout, expected)
        if missing is not None:
            failures.append(f"docs/examples/{name}.md: output absent or out of order:\n{missing}"
                            f"the program printed:\n{run.stdout}")
        else:
            print(f"docs/examples/{name}.md: ok")


def check_commands(failures):
    binary = ROOT / "target" / "release" / "tididi"
    if not binary.exists():
        failures.append("target/release/tididi is missing: run cargo build --release first")
        return
    env = dict(os.environ, PATH=f"{binary.parent}{os.pathsep}{os.environ['PATH']}")
    for page in [ROOT / "README.md", *sorted((ROOT / "docs").glob("*.md"))]:
        markdown = page.read_text(encoding="utf-8")
        checked = 0
        for stream, language, command, expected in output_blocks(markdown):
            if language != "sh":
                continue
            with tempfile.TemporaryDirectory() as scratch:
                for shared in ("tests", "target"):
                    os.symlink(ROOT / shared, Path(scratch) / shared)
                run = subprocess.run(["sh", "-c", command], cwd=scratch, env=env,
                                     capture_output=True, text=True)
            actual = run.stdout if stream == "stdout" else run.stderr
            if actual != expected:
                failures.append(f"{page.relative_to(ROOT)}: `{command.strip()}` printed on {stream}\n"
                                f"{actual}instead of\n{expected}")
            checked += 1
        if checked:
            print(f"{page.relative_to(ROOT)}: {checked} command(s) ok")


def self_test():
    page = ("```sh\ntididi a.cnf\n```\n\nOutput:\n\n```text\nmodel count: 2\n```\n\n```text\nnot output\n```\n"
            "\n```sh\ntididi b.cnf\n```\n\nStderr:\n\n```text\nerror: no\n```\n")
    assert list(output_blocks(page)) == [("stdout", "sh", "tididi a.cnf\n", "model count: 2\n"),
                                         ("stderr", "sh", "tididi b.cnf\n", "error: no\n")]
    assert contains_in_order("a\nb\nc\n", ["a\n", "c\n"]) is None
    assert contains_in_order("a\nb\nc\n", ["c\n", "a\n"]) == "a\n"
    assert contains_in_order("ab\n", ["b\n"]) == "b\n", "a block matches whole lines only"
    print("self-test: ok")


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return
    if sys.argv[1:]:
        sys.exit(__doc__)
    failures = []
    check_walkthroughs(failures)
    check_commands(failures)
    for failure in failures:
        print(failure, file=sys.stderr)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
