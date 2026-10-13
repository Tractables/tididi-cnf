#!/usr/bin/env python3
"""Check that every file naming tididi-bonsai's release version names the same one.

    check-versions.py [tag]

The package version in Cargo.toml is the reference. The package's entry in
Cargo.lock and the `version` in CITATION.cff must equal it, and so must the
newest released section of CHANGELOG.md, its first `## ` heading other than
`Unreleased`, once there is one. With a tag (as the release workflow passes
it, e.g. `v0.1.0`), the tag must name the version and the changelog must have
its section. Prints each disagreement and exits 1.
"""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = "tididi-bonsai"


def text(name):
    return (ROOT / name).read_text(encoding="utf-8")


def main():
    if len(sys.argv) > 2:
        sys.exit(__doc__)
    tag = sys.argv[1] if len(sys.argv) == 2 else ""

    version = tomllib.loads(text("Cargo.toml"))["package"]["version"]
    locked = [p["version"] for p in tomllib.loads(text("Cargo.lock"))["package"]
              if p["name"] == PACKAGE]
    cited = re.findall(r'^version: *"?([^"\s]+)"?\s*$', text("CITATION.cff"), re.M)
    released = [heading for heading in re.findall(r"^## +(.+?)\s*$", text("CHANGELOG.md"), re.M)
                if heading != "Unreleased"]

    problems = []
    if locked != [version]:
        problems.append(f"Cargo.lock names {PACKAGE} {locked}, but Cargo.toml names {version}")
    if cited != [version]:
        problems.append(f"CITATION.cff names {cited}, but Cargo.toml names {version}")
    if released and released[0] != version:
        problems.append(f"CHANGELOG.md's newest release is {released[0]}, but Cargo.toml names {version}")
    if tag and tag != f"v{version}":
        problems.append(f"tag {tag} does not name version {version} (expected v{version})")
    if tag and not released:
        problems.append(f"CHANGELOG.md has no section for {version}")

    if problems:
        print("\n".join(problems))
        sys.exit(1)
    print(f"Cargo.toml, Cargo.lock, CITATION.cff, CHANGELOG.md{' and tag ' + tag if tag else ''} "
          f"name version {version}")


if __name__ == "__main__":
    main()
