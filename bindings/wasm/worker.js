"use strict";

// tididi-cnf in a worker of its own, so the page stays live while a
// formula is counted and a run can be stopped: the page ends this worker and
// starts another. In, one message per run: {dimacs, mode, timeLimitMs}. Out:
// {ready: true} once the module is up, then per run either
// {result: {mode, satisfiable, count, value, stats}, log, elapsed, drawing}
// or {error: {kind, message}, log}. `log` is what the run wrote to stderr, the
// progress lines the binary prints there, and `drawing` what the module's
// draw call returns for the same formula once it is counted.
//
// The page passes its version stamp in this worker's address (see app.js);
// the module and its wasm file take the same stamp.
//
// A throw out of the module is left to reach onerror, where the page
// terminates this worker and starts another: an Emscripten abort poisons the
// module, so every later run in the same worker would throw too.
importScripts("abi.js" + location.search, "tididi_cnf.js" + location.search);

let compiler = null;
let log = [];

createTididiCnf({
  locateFile: (file) => file + location.search,
  printErr: (line) => log.push(line),
}).then(
  (module) => {
    compiler = tididiCnfAbi(module);
    postMessage({ ready: true });
  },
  (failure) => {
    postMessage({ error: { kind: "load", message: String(failure) }, log });
  },
);

onmessage = (event) => {
  const { dimacs, mode, timeLimitMs } = event.data;
  log = [];
  const started = performance.now();
  const answer = compiler.count(dimacs, mode, timeLimitMs);
  const elapsed = performance.now() - started;
  if (answer.ok === true) {
    const { mode, satisfiable, count, value, stats } = answer;
    const countLog = log;
    // What the drawing writes to stderr is not the count's progress.
    log = [];
    const drawing = compiler.draw(dimacs);
    postMessage({ result: { mode, satisfiable, count, value, stats }, log: countLog, elapsed, drawing });
  } else {
    postMessage({ error: answer.error, log });
  }
};
