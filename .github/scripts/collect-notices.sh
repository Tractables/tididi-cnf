#!/usr/bin/env bash
# Copy the licence texts NOTICE names into one directory, for a build that is
# distributed. The release archive and the Python wheel each carry it.
#
#     collect-notices.sh <out-dir> [gmp-prefix]
#     collect-notices.sh list [gmp]
#
# <out-dir> receives vitri/, vitri's texts and its native stack's, which
# vitri's own script collects; tididi/, the diagram library's licence; and
# jemalloc/, the copyright notice of the jemalloc that Cargo.lock's
# tikv-jemalloc-sys builds in. With <gmp-prefix>, where vitri's gmp.sh
# installed GMP, vitri/ also receives GMP's licence texts, for a build that
# ships the GMP libraries.
#
# The `list` form names the files the first form writes, relative to
# <out-dir>, one per line; `gmp` adds the GMP texts.
#
# Needs cargo and python3, which locate the dependencies.
set -euo pipefail

usage() {
  sed -n '2,/^set/{/^set/d;s/^# \{0,1\}//;p}' "$0" >&2
  exit 2
}

here=$(cd "$(dirname "$0")" && pwd)
vitri=$(python3 "$here/dependency-path.py" vitri)

if [ "${1:-}" = list ]; then
  [ "$#" -le 2 ] || usage
  bash "$vitri/.github/scripts/collect-notices.sh" list ${2:+"$2"} | sed 's|^|vitri/|'
  printf '%s\n' tididi/LICENSE jemalloc/COPYING
  exit 0
fi

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  usage
fi
out=$1
tididi=$(python3 "$here/dependency-path.py" tididi)
jemalloc=$(python3 "$here/dependency-path.py" tikv-jemalloc-sys)

bash "$vitri/.github/scripts/collect-notices.sh" "$out/vitri" ${2:+"$2"}
mkdir -p "$out/tididi" "$out/jemalloc"
cp "$tididi/LICENSE" "$out/tididi/"
cp "$jemalloc/jemalloc/COPYING" "$out/jemalloc/"
