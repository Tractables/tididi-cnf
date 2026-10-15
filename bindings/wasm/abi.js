// The function bonsai.wasm exports, called one way from the page's worker and
// from the node smoke test. Loads as a classic script (`importScripts`), which
// defines `bonsaiAbi`, and as a CommonJS module.
//
// Emscripten returns a pointer above 2 GB as a negative number; `>>> 0` reads
// it back as the unsigned address it is.
"use strict";

// The exports of an instantiated `bonsai` module as one call that takes and
// returns JSON values. The string the module returns is allocated on its side
// and released here.
function bonsaiAbi(bonsai) {
  // `bytes` copied into the module's heap while `body` runs with their
  // address. Passed as a "string" argument, ccall would copy them onto the
  // module's stack, which a large formula overflows.
  function lend(bytes, body) {
    const at = bonsai._malloc(Math.max(1, bytes.length)) >>> 0;
    try {
      bonsai.HEAPU8.set(bytes, at);
      return body(at);
    } finally {
      bonsai._free(at);
    }
  }

  // The JSON value of the string a call returned at `pointer`, which this
  // releases.
  function answer(pointer) {
    const text = bonsai.UTF8ToString(pointer >>> 0);
    bonsai._bonsai_string_free(pointer >>> 0);
    return JSON.parse(text);
  }

  return {
    // The DIMACS text `dimacs` counted under `mode`, a mode's token or null
    // for the task its header declares, within `timeLimitMs` milliseconds, or
    // none for 0: {ok: true, mode, satisfiable, count, value, stats} or
    // {ok: false, error: {kind, message}}.
    count(dimacs, mode, timeLimitMs) {
      const encoder = new TextEncoder();
      const formula = encoder.encode(dimacs);
      const modeText = mode === null ? null : encoder.encode(mode + "\0");
      return lend(formula, (formulaAt) => {
        const call = (modeAt) => answer(bonsai._bonsai_count_json(formulaAt, formula.length, modeAt, timeLimitMs));
        return modeText === null ? call(0) : lend(modeText, call);
      });
    },

    // The steps of compiling the DIMACS text `dimacs` bottom-up over a vtree,
    // for a formula of a few variables: {ok: true, clauses, weights, vtree,
    // steps} or {ok: false, error: {kind, message}}.
    draw(dimacs) {
      const formula = new TextEncoder().encode(dimacs);
      return lend(formula, (formulaAt) => answer(bonsai._bonsai_draw_json(formulaAt, formula.length)));
    },
  };
}

if (typeof module !== "undefined") module.exports = { bonsaiAbi };
