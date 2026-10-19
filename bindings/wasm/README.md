# tididi-cnf in the browser

**<https://tractables.github.io/tididi-cnf/>**

A page that runs tididi-cnf in the tab. Drop in a DIMACS CNF — or paste
one, open a file, or pick an example — and it counts the formula's models:

- the model count, the weighted count, or either one projected onto the
  formula's show variables, as its `c t` line declares or as the task picked
  on the page, with the weights and the show variables read from its
  `c p weight` and `c p show` lines
  ([input reference](../../docs/input.md), [tasks](../../docs/tasks.md)); a
  weighted count has its decimal value beside it,
- whether the formula has a model, which stage answered, and the time it took,
- the run's record, as the binary's `--stats` writes it
  ([`stats.md`](../../docs/stats.md)), and the progress the binary prints on
  stderr,
- for a formula of a few variables, its compile, drawn step by step: the
  vtree, each clause at the lowest vtree node whose subtree holds its
  variables, and the diagram built at each node, every node with its count;
  then each variable's probability, or its share of the models, under
  evidence pinned on the page, evaluated from the diagram.

The count is the one the `tididi` binary prints on the same formula and task:
the page calls the crate's `tididi_cnf::count`. A time limit is the
binary's `--time-limit-ms`. There is no memory limit: the module's memory
grows as the run needs it, up to 4 GB, and the crate refuses
`Limits::mem_limit_mb` on WebAssembly. The address bar carries the example and
the settings, so a result can be linked to. Nothing is uploaded and nothing is
fetched from another site: the formula never leaves the browser.

Editing the formula or settings while a run is in progress leaves its
answer and drawing marked as belonging to the previous input. Count again
to update them.

The drawing compiles the formula as it is written, without the preprocessing
a count runs, on the vtree vitri's portfolio builds for it: one compile per
internal vtree node, of the clauses whose variables all lie under it. The
module returns each diagram in the library's `.tdd` text format, and
`tdd.js` reads and evaluates it; `picture.js` draws it, and
`compile-view.js` runs the card the drawing is on.

## Build and serve it locally

The module builds the way vitri's own browser build does, with the Emscripten
SDK on `PATH`, the `wasm32-unknown-emscripten` target, and GMP and MPFR built
for Emscripten into a prefix that `VITRI_EMSCRIPTEN_PREFIX` names
([vitri's building guide](https://github.com/Tractables/vitri/blob/main/docs/building.md#building-for-emscripten)).
vitri's `emscripten-prefix.sh` builds that prefix:

```sh
rustup target add wasm32-unknown-emscripten
vendor/vitri/bindings/wasm/emscripten-prefix.sh target/gmp target/emscripten-prefix
export VITRI_EMSCRIPTEN_PREFIX=$PWD/target/emscripten-prefix
(cd bindings/wasm && cargo build --release)
bindings/wasm/site.sh bindings/wasm/site "$VITRI_EMSCRIPTEN_PREFIX"
python3 -m http.server -d bindings/wasm/site
```

`site.sh` gathers what the page is served with, including the example formulas
`examples.js` names, which it copies from `tests/data/`, and puts the
[evaluation page](../../docs/evaluation/index.html) and the results it reads
under `evaluation/`; the web build workflow builds its artifact the same way.
Opening `index.html` from the filesystem does not work: the module and the
examples are fetched, so they have to come from a server.

`smoke.mjs` runs the built module on the page's examples and on the
instances [`bindings/parity.txt`](../parity.txt) lists, and compares each
answer with the native binary's on the same file, then draws the small
examples and checks each step's diagram against enumeration; vitri's
`page-check.mjs` checks `index.html` against `styles.css`, and the
evaluation page's `index.html` against `page.css`. The web build workflow
runs these checks and `app-test.mjs`, which checks delayed worker replies
after input and settings edits without building the module.

The native binary is built without `VITRI_EMSCRIPTEN_PREFIX` in the
environment, which a native build of vitri refuses:

```sh
node --experimental-vm-modules bindings/wasm/app-test.mjs
env -u VITRI_EMSCRIPTEN_PREFIX cargo build --release --bin tididi
node bindings/wasm/smoke.mjs bindings/wasm/site/tididi_cnf.js . target/release/tididi
node vendor/vitri/bindings/wasm/page-check.mjs bindings/wasm/index.html bindings/wasm/styles.css
node vendor/vitri/bindings/wasm/page-check.mjs docs/evaluation/index.html docs/evaluation/page.css
```

## Cache stamps

A workflow that publishes the page stamps the two references in `index.html`
and the imports between the scripts with the commit (`app.js?v=<sha>`). A
script reads its own stamp from `import.meta.url`; the page passes it to the
worker, the worker to `tididi_cnf.js` and every file the module loads, and the
example list to the formulas it fetches, so a new page never runs with an
older file a browser still holds. The evaluation page under `evaluation/` is
stamped the same way, and its results take the stamp of the script that
fetches them.
