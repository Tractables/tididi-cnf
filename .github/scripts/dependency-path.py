#!/usr/bin/env python3
"""Print a dependency's source directory from the committed Cargo resolution."""

import json
from pathlib import Path
import subprocess
import sys

if len(sys.argv) != 2:
    sys.exit("usage: dependency-path.py <package>")
root = Path(__file__).resolve().parents[2]
result = subprocess.run(
    ["cargo", "metadata", "--locked", "--format-version", "1"],
    cwd=root, check=True, stdout=subprocess.PIPE, text=True,
)
packages = [p for p in json.loads(result.stdout)["packages"]
            if p["name"] == sys.argv[1]]
if len(packages) != 1:
    sys.exit(f"expected one {sys.argv[1]} package, found {len(packages)}")
print(Path(packages[0]["manifest_path"]).parent)
