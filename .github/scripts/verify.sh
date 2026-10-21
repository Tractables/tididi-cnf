#!/usr/bin/env bash
# The verification sequence for contributors and CI. Cargo runs `--locked`,
# against the committed Cargo.lock. Each command has a 30-minute limit; stop on
# the first failure and name it.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

step() {
  local label=$1
  shift
  printf '\n=== %s ===\n' "$label"
  if timeout --kill-after=30s 1800 "$@"; then
    printf '=== PASS: %s ===\n' "$label"
  else
    local status=$?
    printf '=== FAIL: %s (exit %s) ===\n' "$label" "$status" >&2
    return "$status"
  fi
}

printf 'Verification source: %s\n' "$(git rev-parse HEAD 2>/dev/null || printf 'unavailable')"
step 'Debug build' cargo build --locked
step 'Release build' cargo build --release --locked
step 'Clippy' cargo clippy --all-targets -p tididi-cnf --locked -- -D warnings
step 'Rustdoc' env RUSTDOCFLAGS="${RUSTDOCFLAGS:+$RUSTDOCFLAGS }-D warnings" \
  cargo doc --no-deps -p tididi-cnf --locked
# One test at a time, for the reason .github/CONTRIBUTING.md § Checks gives.
step 'Tests' env RUST_BACKTRACE=full \
  cargo test --all-targets --locked -- --test-threads=1
step 'Doctests' cargo test --doc --locked
step 'Doc checker self-test' python3 .github/scripts/check_docs.py --self-test
step 'Documented output' python3 .github/scripts/check_docs.py
printf '\n=== Verification passed ===\n'
