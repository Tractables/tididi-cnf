#!/usr/bin/env bash
# Put the files the browser page is served with into one directory.
#
#     site.sh <out-dir> [emscripten-prefix]
#
# <out-dir> receives the page: index.html, styles.css, the scripts, vitri's
# reader of the .vtree format from its Cargo dependency, the
# crate's logo, docs/logo.svg, and the example formulas examples.js names,
# from tests/data/. Its evaluation/ receives the evaluation page from
# docs/evaluation/: index.html, its stylesheet and scripts. Results are fetched
# from the evaluation branch commit pinned in docs/evaluation/revision.
# With <emscripten-prefix>, the prefix vitri's emscripten-prefix.sh installed
# GMP into, it also receives the built module from
# target/wasm32-unknown-emscripten/release, the two side modules the module
# loads from that prefix, and the notices under notices/: tididi-cnf's
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
vitri=$(python3 "$root/.github/scripts/dependency-path.py" vitri)

mkdir -p "$out"
cp "$here"/index.html "$here"/styles.css "$here"/*.js "$root"/docs/logo.svg "$out/"
cp "$vitri/bindings/wasm/vtree.js" "$out/"
mkdir -p "$out/evaluation"
cp "$root"/docs/evaluation/index.html "$root"/docs/evaluation/*.{css,js,svg} "$out/evaluation/"
revision=$(cat "$root/docs/evaluation/revision")
if [[ ! "$revision" =~ ^[0-9a-f]{40}$ ]]; then
  echo "docs/evaluation/revision must pin a full commit SHA" >&2
  exit 1
fi
curl --fail --location --retry 3 --connect-timeout 20 --max-time 120 \
  "https://codeload.github.com/Tractables/tididi-cnf/tar.gz/$revision" |
  tar -xz --strip-components=2 -C "$out/evaluation" --wildcards '*/results/*'
for example in $(grep -o '"[A-Za-z0-9_]*\.cnf"' "$here/examples.js" | tr -d '"'); do
  cp "$root/tests/data/$example" "$out/"
done

if [ "$#" -eq 2 ]; then
  prefix=$(cd "$2" && pwd)
  cp "$here"/target/wasm32-unknown-emscripten/release/tididi_cnf.{js,wasm} \
    "$prefix"/lib/libgmp.so "$prefix"/lib/libgmpxx.so "$out/"
  mkdir -p "$out/notices"
  cp "$root"/LICENSE "$root"/NOTICE "$out/notices/"
  "$vitri/.github/scripts/collect-notices.sh" "$out/notices/vitri" "$prefix"
fi
