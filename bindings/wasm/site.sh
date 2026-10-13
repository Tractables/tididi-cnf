#!/usr/bin/env bash
# Put the files the browser page is served with into one directory.
#
#     site.sh <out-dir> [emscripten-prefix]
#
# <out-dir> receives the page: index.html, styles.css, the scripts, the
# crate's logo, docs/logo.svg, and the example formulas examples.js names,
# from tests/data/.
# With <emscripten-prefix>, the prefix vitri's emscripten-prefix.sh installed
# GMP into, it also receives the built module from
# target/wasm32-unknown-emscripten/release, the two side modules the module
# loads from that prefix, and the notices under notices/: tididi-bonsai's
# LICENSE and NOTICE, and vitri's own under notices/vitri/.
#
# The docs workflow assembles the published site from this directory, and
# bindings/wasm/README.md serves it from a local build.
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  sed -n '2,/^set/{/^set/d;s/^# \{0,1\}//;p}' "$0" >&2
  exit 2
fi

out=$1
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

mkdir -p "$out"
cp "$here"/index.html "$here"/styles.css "$here"/*.js "$root"/docs/logo.svg "$out/"
for example in $(grep -o '"[A-Za-z0-9_]*\.cnf"' "$here/examples.js" | tr -d '"'); do
  cp "$root/tests/data/$example" "$out/"
done

if [ "$#" -eq 2 ]; then
  prefix=$(cd "$2" && pwd)
  cp "$here"/target/wasm32-unknown-emscripten/release/bonsai.{js,wasm} \
    "$prefix"/lib/libgmp.so "$prefix"/lib/libgmpxx.so "$out/"
  mkdir -p "$out/notices"
  cp "$root"/LICENSE "$root"/NOTICE "$out/notices/"
  "$root/vendor/vitri/.github/scripts/collect-notices.sh" "$out/notices/vitri" "$prefix"
fi
