// Delayed worker replies must retain the warning when the input changes.
// Run without a browser or a built module:
//   node --experimental-vm-modules bindings/wasm/app-test.mjs
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

class Element {
  value = "";
  hidden = true;
  children = [];
  dataset = {};
  validity = {};
  options = [];
  listeners = new Map();
  classList = { add() {}, remove() {} };
  addEventListener(kind, callback) { this.listeners.set(kind, callback); }
  emit(kind) { this.listeners.get(kind)?.({ preventDefault() {} }); }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  setAttribute() {}
  querySelector() { return null; }
}

const elements = new Map();
const element = (id) => {
  if (!elements.has(id)) elements.set(id, new Element());
  return elements.get(id);
};
let worker;
class Worker {
  constructor() { worker = this; }
  postMessage(job) { this.job = job; }
  reply(data) { this.onmessage({ data }); }
  terminate() {}
}
const context = vm.createContext({
  document: {
    getElementById: element,
    createElement: () => new Element(),
    createTextNode: (text) => text,
    addEventListener() {},
    body: new Element(),
  },
  Worker, URL, URLSearchParams, TextEncoder,
  location: { search: "", pathname: "/" },
  history: { replaceState() {} },
  performance: { now: () => 0 },
  setInterval() {}, clearInterval() {}, setTimeout() {}, clearTimeout() {},
});
const modules = {
  "./compile-view.js": {
    showDrawing() { element("compile-stale").hidden = true; },
    markDrawingStale() { element("compile-stale").hidden = false; },
  },
  "./examples.js": { EXAMPLES: [["example", "Example", "example.cnf"]], fetchExample: () => new Promise(() => {}) },
  "./words.js": {
    ANSWERED_BY: {}, ANSWER_LABELS: {}, ERROR_KINDS: {}, MODE_NAMES: {},
    duration: String, megabytes: String, plural: String,
  },
};
const sourceModule = (name) => new vm.SourceTextModule(fs.readFileSync(new URL(name, import.meta.url), "utf8"), {
  context,
  initializeImportMeta(meta) { meta.url = new URL(name, import.meta.url).href; },
});
const app = sourceModule("app.js");
await app.link((name) => {
  if (name === "./runner.js") return sourceModule("runner.js");
  const exports = modules[name];
  return new vm.SyntheticModule(Object.keys(exports), function () {
    for (const [key, value] of Object.entries(exports)) this.setExport(key, value);
  }, { context });
});
await app.evaluate();
worker.reply({ ready: true });

const answer = { result: { mode: "mc", satisfiable: true, count: "2", stats: {} }, log: [], elapsed: 1, drawing: {} };
element("cnf").value = "p cnf 1 0\n";
element("settings").emit("submit");
element("cnf").value = "p cnf 1 1\n1 0\n";
element("cnf").emit("input");
worker.reply(answer);
assert.equal(element("stale").hidden, false, "an edit during the first run makes its delayed answer stale");
assert.equal(element("compile-stale").hidden, false, "the delayed drawing is stale too");

element("settings").emit("submit");
worker.reply(answer);
assert.equal(element("stale").hidden, true, "an unchanged submitted input has a current answer");
assert.equal(element("compile-stale").hidden, true, "its drawing is current too");

element("settings").emit("submit");
element("task").value = "wmc";
element("settings").emit("input");
worker.reply({ error: { kind: "usage", message: "old task failed" }, log: [] });
assert.equal(element("stale").hidden, false, "a settings edit also makes a delayed failure stale");
console.log("app-test: delayed answers and failures retain input changes");
