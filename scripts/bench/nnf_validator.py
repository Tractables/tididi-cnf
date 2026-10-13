"""Build and verify the validator from this checkout before running it."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


PROJECT_DIR = Path(__file__).resolve().parents[2]
BUILD_COMMAND = "python3 scripts/bench/nnf_validator.py --build"


class NnfValidatorMissing(RuntimeError):
    """The snapshot's validator is missing, stale, or lacks a build receipt."""


def _paths():
    crate = Path(PROJECT_DIR) / "tools/nnf_validator"
    binary = crate / "target/release/nnf_validator"
    return crate, binary, binary.with_suffix(".build.json")


def _sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _sources():
    """Hash the standalone crate's inputs, independent of Git and timestamps."""
    root = Path(PROJECT_DIR)
    crate, _, _ = _paths()
    files = [crate / "Cargo.toml", crate / "Cargo.lock"]
    files.extend(sorted((crate / "src").rglob("*")))
    for relative in ("tools/nnf_validator/build.rs", "rust-toolchain", "rust-toolchain.toml",
                     ".cargo/config", ".cargo/config.toml"):
        path = root / relative
        if path.is_file():
            files.append(path)
    return {str(path.relative_to(root)): _sha256(path) for path in files if path.is_file()}


def find_nnf_validator():
    """Require this checkout's binary and a matching successful-build receipt."""
    _, binary, receipt = _paths()
    try:
        record = json.loads(receipt.read_text())
        matches = (record.get("version") == 1 and record.get("sources") == _sources()
                   and os.access(binary, os.X_OK)
                   and record.get("binary_sha256") == _sha256(binary))
        if matches:
            return str(binary)
    except (OSError, ValueError, TypeError, AttributeError):
        pass
    raise NnfValidatorMissing(
        f"Matching-snapshot nnf_validator unavailable; run `{BUILD_COMMAND}` "
        f"in {PROJECT_DIR} before tests or benchmarks. Global binaries are not used.")


def build_nnf_validator():
    """Build into the crate's target directory and atomically record its inputs."""
    crate, binary, receipt = _paths()
    target = crate / "target"
    target.mkdir(parents=True, exist_ok=True)
    with (target / ".validator-build.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        receipt.unlink(missing_ok=True)
        sources = _sources()
        common = ["--manifest-path", str(crate / "Cargo.toml"), "--target-dir", str(target)]
        # A synced checkout can preserve old mtimes. Recompile this crate while
        # keeping dependency artifacts, rather than trusting Cargo's timestamps.
        subprocess.run(["cargo", "clean", "--package", "nnf_validator", *common],
                       cwd=PROJECT_DIR, check=True)
        command = ["cargo", "build", "--locked", "--release", *common]
        subprocess.run(command, cwd=PROJECT_DIR, check=True)
        if sources != _sources():
            raise RuntimeError("Validator inputs changed during the build; rebuild the stable snapshot")
        record = {"version": 1, "sources": sources, "binary_sha256": _sha256(binary),
                  "command": command, "rustc": subprocess.check_output(
                      ["rustc", "--version"], text=True).strip()}
        pending = receipt.with_suffix(".pending")
        pending.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
        pending.replace(receipt)
    return find_nnf_validator()


def run_nnf_validator(nnf_path, convert_path=None, timeout=60):
    """Run nnf_validator on an NNF file and return parsed metrics dict.

    Returns dict with keys: model_count, nodes, edges, vars,
    solver_declared_vars, and_nodes, or_nodes, leaf_nodes.
    Raises NnfValidatorMissing if the binary isn't installed; raises
    RuntimeError on any other failure (timeout, non-zero exit, malformed JSON).
    """
    binary = find_nnf_validator()
    cmd = [binary, "--json", nnf_path]
    if convert_path:
        cmd += ["--convert", convert_path]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise RuntimeError(f"nnf_validator timed out after {timeout}s")
    if proc.returncode != 0:
        raise RuntimeError(f"nnf_validator failed (rc={proc.returncode}): {proc.stderr.strip()}")
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        raise RuntimeError(f"nnf_validator produced invalid JSON: {proc.stdout[:200]}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--build", action="store_true", help="build and record this snapshot's validator")
    action.add_argument("--path", action="store_true", help="verify the receipt and print the binary path")
    args = parser.parse_args()
    try:
        print(build_nnf_validator() if args.build else find_nnf_validator())
    except (NnfValidatorMissing, RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
