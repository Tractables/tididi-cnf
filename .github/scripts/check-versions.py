#!/usr/bin/env python3
"""Check that every file naming tididi-bonsai's release version names the same one.

    check-versions.py [tag]

The package version in Cargo.toml is the reference. The package's entry in
Cargo.lock and the `version` in CITATION.cff must equal it, and so must the
newest released section of CHANGELOG.md, its first `## ` heading other than
`Unreleased`, once there is one. Each binding under bindings/ is a package of
its own that reports the library's version, so its Cargo.toml must name it
too, and its Cargo.lock must lock both the binding and the library at it.
With a tag (as the release workflow passes it, e.g. `v0.1.0`), the tag must
name the version and the changelog must have its section. Prints each
disagreement and exits 1.
"""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = "tididi-bonsai"
BINDINGS = ["bindings/c", "bindings/python", "bindings/wasm"]


def text(name):
    return (ROOT / name).read_text(encoding="utf-8")


def locked(lockfile, package):
    """The versions `lockfile` locks `package` at."""
    return [p["version"] for p in tomllib.loads(text(lockfile))["package"] if p["name"] == package]


def main():
    if len(sys.argv) > 2:
        sys.exit(__doc__)
    tag = sys.argv[1] if len(sys.argv) == 2 else ""

    version = tomllib.loads(text("Cargo.toml"))["package"]["version"]
    named = {
        f"Cargo.lock's {PACKAGE}": locked("Cargo.lock", PACKAGE),
        "CITATION.cff": re.findall(r'^version: *"?([^"\s]+)"?\s*$', text("CITATION.cff"), re.M),
    }
    for binding in BINDINGS:
        package = tomllib.loads(text(f"{binding}/Cargo.toml"))["package"]
        named[f"{binding}/Cargo.toml"] = [package["version"]]
        for name in (package["name"], PACKAGE):
            named[f"{binding}/Cargo.lock's {name}"] = locked(f"{binding}/Cargo.lock", name)
    released = [heading for heading in re.findall(r"^## +(.+?)\s*$", text("CHANGELOG.md"), re.M)
                if heading != "Unreleased"]

    problems = [f"{source} names {found}, but Cargo.toml names {version}"
                for source, found in named.items() if found != [version]]
    if released and released[0] != version:
        problems.append(f"CHANGELOG.md's newest release is {released[0]}, but Cargo.toml names {version}")
    if tag and tag != f"v{version}":
        problems.append(f"tag {tag} does not name version {version} (expected v{version})")
    if tag and not released:
        problems.append(f"CHANGELOG.md has no section for {version}")

    if problems:
        print("\n".join(problems))
        sys.exit(1)
    print(f"Cargo.toml, Cargo.lock, CITATION.cff, CHANGELOG.md, the bindings' manifests and lockfiles"
          f"{' and tag ' + tag if tag else ''} name version {version}")


if __name__ == "__main__":
    main()
