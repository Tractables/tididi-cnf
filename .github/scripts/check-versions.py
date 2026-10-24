#!/usr/bin/env python3
"""Check that every file naming tididi-cnf's release version names the same one.

    check-versions.py [tag]

The package version in Cargo.toml is the reference. These must equal it:

- the package's entry in Cargo.lock;
- the version of each binding, `bindings/*/Cargo.toml`: a package of its own
  that reports the library's version, and whose Cargo.lock must lock both
  the binding and the library at it;
- the `version` in CITATION.cff;
- every link a worked example, `docs/examples/*.md`, makes to its program at
  a release tag (`tididi-cnf/blob/v<version>/`);
- the C package the C binding's README unpacks (`tididi-cnf-c-v<version>-`).

With a tag (as the release workflow passes it, e.g. `v0.1.0`), the tag must
name the version. Prints each disagreement and exits 1.
"""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = "tididi-cnf"
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
    linked = [(page.name, tagged)
              for page in sorted((ROOT / "docs/examples").glob("*.md"))
              for tagged in re.findall(r"tididi-cnf/blob/v([^/]+)/", page.read_text(encoding="utf-8"))]
    packaged = re.findall(r"tididi-cnf-c-v([^-\s]+)-", text("bindings/c/README.md"))

    problems = [f"{source} names {found}, but Cargo.toml names {version}"
                for source, found in named.items() if found != [version]]
    for page, tagged in linked:
        if tagged != version:
            problems.append(f"docs/examples/{page} links its program at v{tagged}, but Cargo.toml names {version}")
    for found in packaged:
        if found != version:
            problems.append(f"bindings/c/README.md unpacks the C package of v{found}, but Cargo.toml names {version}")
    if tag and tag != f"v{version}":
        problems.append(f"tag {tag} does not name version {version} (expected v{version})")

    if problems:
        print("\n".join(problems))
        sys.exit(1)
    print(f"Cargo.toml, the lockfiles, the bindings, CITATION.cff, the worked examples' "
          f"links{', the C README and tag ' + tag if tag else ' and the C README'} name version {version}")


if __name__ == "__main__":
    main()
