// The page: the input, the settings, the run, and the result. The worker that
// runs tididi-bonsai is worker.js, and runner.js drives it; compile-view.js
// draws the compile of a small formula.

import { markDrawingStale, showDrawing } from "./compile-view.js";
import { EXAMPLES, fetchExample } from "./examples.js";
import { Runner } from "./runner.js";
import { ANSWERED_BY, ANSWER_LABELS, ERROR_KINDS, MODE_NAMES, duration, megabytes, plural } from "./words.js";

// An input past this size stays out of the text box, which becomes slow on
// multi-megabyte text; it is held in memory and sent as it is.
const HELD_INPUT_BYTES = 1024 * 1024;

// This file's version stamp (see README.md), passed on to the worker.
const stamp = new URL(import.meta.url).search;

const element = (id) => document.getElementById(id);
const make = (name, className, text) => {
  const node = document.createElement(name);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const cnfBox = element("cnf");
const placeholder = cnfBox.placeholder;
const statusLine = element("status");
const control = { task: element("task"), limit: element("limit") };

// The input: the text box, or a large file held in memory with its name.
let held = null;
let inputName = null;
const inputText = () => (held === null ? cnfBox.value : held.text);

// Whether a result is on show.
let shown = false;

// The declared size of a DIMACS text, from its problem line.
function dimacsHeader(text) {
  const match = /^p\s+cnf\s+(\d+)\s+(\d+)/m.exec(text.slice(0, 1 << 20));
  return match === null ? null : { variables: Number(match[1]), clauses: Number(match[2]) };
}

// ------------------------------------------------------------- examples

const examples = element("examples");
EXAMPLES.forEach(([, name], i) => {
  const chip = make("button", "", name);
  chip.type = "button";
  chip.dataset.example = String(i);
  chip.setAttribute("aria-pressed", "false");
  examples.append(chip);
});
const markExample = (i) => {
  for (const chip of examples.children) chip.setAttribute("aria-pressed", String(chip.dataset.example === String(i)));
};
const chosenExample = () => {
  const chip = examples.querySelector('[aria-pressed="true"]');
  return chip === null ? -1 : Number(chip.dataset.example);
};

let loading = 0;
async function loadExample(i) {
  const ticket = ++loading;
  markExample(i);
  let text;
  try {
    text = await fetchExample(EXAMPLES[i][2]);
  } catch (failure) {
    statusLine.textContent = `The example did not load: ${failure.message ?? failure}`;
    return;
  }
  if (ticket !== loading) return;
  setInput(text, EXAMPLES[i][2]);
  markExample(i);
  requestRun();
}
examples.addEventListener("click", (event) => {
  const chip = event.target.closest("button");
  if (chip !== null) loadExample(Number(chip.dataset.example));
});

// ---------------------------------------------------------------- input

function setInput(text, name) {
  inputName = name;
  if (text.length > HELD_INPUT_BYTES) {
    held = { text };
    cnfBox.value = "";
    cnfBox.disabled = true;
    cnfBox.placeholder = "This file is too large to show here. It is held in memory and will be sent as it is; Clear removes it.";
    element("clear").hidden = false;
  } else {
    held = null;
    cnfBox.disabled = false;
    cnfBox.value = text;
    cnfBox.placeholder = placeholder;
    element("clear").hidden = true;
  }
  markExample(-1);
  describeInput();
  markStale();
}

function describeInput() {
  const text = inputText();
  const parts = [];
  if (inputName !== null) parts.push(inputName);
  if (held !== null || text.length > 64 * 1024) parts.push(megabytes(new TextEncoder().encode(text).length));
  const header = dimacsHeader(text);
  if (header !== null) parts.push(`${plural(header.variables, "variable")}, ${plural(header.clauses, "clause")} declared`);
  element("input-info").textContent = parts.join(", ");
}

let typing = 0;
cnfBox.addEventListener("input", () => {
  inputName = null;
  markExample(-1);
  markStale();
  clearTimeout(typing);
  typing = setTimeout(describeInput, 300);
});

element("clear").addEventListener("click", () => {
  setInput("", null);
  cnfBox.focus();
});

// A gzip file is decompressed in the tab; other compressed formats are
// refused by name rather than sent as bytes.
async function readFile(file) {
  const head = new Uint8Array(await file.slice(0, 6).arrayBuffer());
  const starts = (...bytes) => bytes.every((byte, i) => head[i] === byte);
  if (starts(0x1f, 0x8b)) {
    if (typeof DecompressionStream === "undefined") throw new Error("this browser cannot decompress gzip");
    return new Response(file.stream().pipeThrough(new DecompressionStream("gzip"))).text();
  }
  if (starts(0xfd, 0x37, 0x7a, 0x58, 0x5a)) throw new Error("it is xz-compressed; decompress it first");
  if (starts(0x42, 0x5a, 0x68)) throw new Error("it is bzip2-compressed; decompress it first");
  if (starts(0x50, 0x4b, 0x03, 0x04)) throw new Error("it is a zip archive; extract the CNF first");
  return file.text();
}

async function loadFile(file) {
  if (file === undefined) return;
  const ticket = ++loading;
  let text;
  try {
    text = await readFile(file);
  } catch (failure) {
    statusLine.textContent = `${file.name} was not read: ${failure.message ?? failure}`;
    return;
  }
  if (ticket !== loading) return;
  setInput(text, file.name);
  requestRun();
}

const fileInput = element("file");
element("open").addEventListener("click", () => fileInput.click());
fileInput.addEventListener("change", () => {
  loadFile(fileInput.files[0]);
  fileInput.value = "";
});

// The whole page takes a drop. Dragging over child elements fires leave and
// enter in pairs, so a count tells when the file has left the page.
let dragging = 0;
const hasFiles = (event) => event.dataTransfer !== null && Array.from(event.dataTransfer.types).includes("Files");
document.addEventListener("dragenter", (event) => {
  if (!hasFiles(event)) return;
  event.preventDefault();
  dragging += 1;
  document.body.classList.add("dropping");
});
document.addEventListener("dragover", (event) => {
  if (!hasFiles(event)) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "copy";
});
document.addEventListener("dragleave", (event) => {
  if (!hasFiles(event)) return;
  dragging = Math.max(0, dragging - 1);
  if (dragging === 0) document.body.classList.remove("dropping");
});
document.addEventListener("drop", (event) => {
  if (!hasFiles(event)) return;
  event.preventDefault();
  dragging = 0;
  document.body.classList.remove("dropping");
  loadFile(event.dataTransfer.files[0]);
});

// ------------------------------------------------------------- settings

// The time limit `text` sets, in milliseconds: 0 for none when it is empty,
// or null when it is not a whole number the module takes, which is 1 or more
// and fits in 32 bits.
function timeLimit(text) {
  if (text === "") return 0;
  const ms = Number(text);
  return Number.isInteger(ms) && ms >= 1 && ms <= 0xffffffff ? ms : null;
}

// The job for the worker: the mode token, or null for the one the formula
// declares, and the time limit, 0 for none. Returns {job} or {error}.
function buildJob() {
  const timeLimitMs = control.limit.validity.badInput ? null : timeLimit(control.limit.value.trim());
  if (timeLimitMs === null) {
    return { error: "The time limit is a whole number of milliseconds, 1 or more. Leave it empty for none." };
  }
  return { job: { dimacs: inputText(), mode: control.task.value === "" ? null : control.task.value, timeLimitMs } };
}

element("settings").addEventListener("input", markStale);
element("settings").addEventListener("submit", (event) => {
  event.preventDefault();
  requestRun();
});

// The address carries the example and the settings, so a result can be
// linked to. It is read once when the page opens and written at every run.
// A formula typed or opened is not in the address, so a run on one keeps
// only the settings there and hides the button that copies it.
function writeAddress() {
  const params = new URLSearchParams();
  const example = chosenExample();
  if (example >= 0) params.set("example", EXAMPLES[example][0]);
  if (control.task.value !== "") params.set("task", control.task.value);
  if (control.limit.value.trim() !== "") params.set("limit", control.limit.value.trim());
  const query = params.toString();
  history.replaceState(null, "", query === "" ? location.pathname : `${location.pathname}?${query}`);
  element("link").hidden = example < 0;
}

function readAddress() {
  const params = new URLSearchParams(location.search);
  const task = params.get("task");
  if (task !== null && [...control.task.options].some((item) => item.value === task)) control.task.value = task;
  // A limit the settings would refuse is left out.
  const limit = params.get("limit");
  if (limit !== null && timeLimit(limit)) control.limit.value = limit;
  // An address that names no example, or one the row does not have, opens
  // on the first.
  const example = EXAMPLES.findIndex(([key]) => key === params.get("example"));
  loadExample(Math.max(0, example));
}

function flash(button, text) {
  const label = button.textContent;
  button.textContent = text;
  setTimeout(() => {
    button.textContent = label;
  }, 1500);
}
element("link").addEventListener("click", () => {
  navigator.clipboard.writeText(location.href).then(
    () => flash(element("link"), "Copied"),
    () => flash(element("link"), "Not copied"),
  );
});

// ---------------------------------------------------------------- runs

let ticking = 0;

const runner = new Runner("worker.js" + stamp, {
  loading() {
    element("run").disabled = true;
  },
  ready() {
    element("run").disabled = false;
    statusLine.textContent = "Ready.";
  },
  notLoaded(message) {
    statusLine.textContent = `tididi-bonsai did not load: ${message}`;
  },
  started() {
    const started = performance.now();
    statusLine.textContent = "Counting.";
    ticking = setInterval(() => {
      statusLine.textContent = `Counting, ${Math.round((performance.now() - started) / 1000)} s.`;
    }, 1000);
    element("run").hidden = true;
    element("cancel").hidden = false;
    document.body.classList.add("busy");
  },
  done(result, log, elapsed, drawing) {
    settle();
    showAnswer(result, log, elapsed);
    showDrawing(drawing);
  },
  failed(error, log) {
    settle();
    showFailure(error, log);
    showDrawing(null);
  },
});

// The page after a run, whichever way it ended.
function settle() {
  clearInterval(ticking);
  document.body.classList.remove("busy");
  element("cancel").hidden = true;
  element("run").hidden = false;
}

function requestRun() {
  if (inputText().trim() === "") {
    statusLine.textContent = "Paste, open or drop a DIMACS CNF first.";
    return;
  }
  const built = buildJob();
  if (built.error !== undefined) {
    statusLine.textContent = built.error;
    return;
  }
  writeAddress();
  if (runner.running) settle();
  if (!runner.ready) statusLine.textContent = "Loading tididi-bonsai; the run starts when it is ready.";
  runner.run(built.job);
}

element("cancel").addEventListener("click", () => {
  if (!runner.cancel()) return;
  settle();
  statusLine.textContent = "Stopped. Loading tididi-bonsai again.";
});

function markStale() {
  if (shown) element("stale").hidden = false;
  markDrawingStale();
}

// ---------------------------------------------------------------- result

function showAnswer(result, log, elapsed) {
  shown = true;
  element("stale").hidden = true;
  statusLine.textContent = `Done in ${duration(elapsed)}.`;
  const { mode, satisfiable, count, value, stats } = result;
  const answer = make("div", satisfiable ? "answer" : "answer unsatisfiable");
  answer.append(make("span", "label", ANSWER_LABELS[mode] ?? mode), make("b", "", count));
  const approximate = decimal(mode, count, value);
  if (approximate !== null) answer.append(make("span", "decimal", `≈ ${approximate}`));
  const facts = make("dl", "facts");
  const fact = (term, value) => facts.append(make("dt", "", term), make("dd", "", value));
  fact("Task", `${mode}: ${MODE_NAMES[mode] ?? mode}`);
  fact("Satisfiable", satisfiable ? "yes" : "no");
  if (stats.variables !== undefined) {
    fact("Formula", `${plural(stats.variables, "variable")}, ${plural(stats.clauses ?? 0, "clause")}`);
  }
  if (stats.answered_by !== undefined) fact("Answered by", ANSWERED_BY[stats.answered_by] ?? stats.answered_by);
  if (stats.vtree_candidate !== undefined) fact("Vtree", `${stats.vtree_spec}, won by ${stats.vtree_candidate}`);
  fact("Time", `${duration(elapsed)} in this tab`);
  element("summary").replaceChildren(answer, facts);
  element("record").textContent = JSON.stringify(stats, null, 2);
  element("record-box").hidden = false;
  showLog(log);
}

// A weighted count, a rational or a log-domain value, has its nearest double
// beside it to six significant digits, unless that reads as the count does.
// null when there is none to show: an unweighted task, or a value past a
// double's range, which arrives as null.
function decimal(mode, count, value) {
  if ((mode !== "wmc" && mode !== "pwmc") || typeof value !== "number") return null;
  const text = String(Number(value.toPrecision(6)));
  return text === count ? null : text;
}

function showFailure(error, log) {
  shown = true;
  element("stale").hidden = true;
  const kind = error && typeof error.kind === "string" ? error.kind : "worker";
  const message = error && typeof error.message === "string" ? error.message : String(error);
  statusLine.textContent = "No count; see the result.";
  const box = make("div", "answer failed");
  const heading = make("p");
  heading.append(make("strong", "", "No count: "), document.createTextNode(`${ERROR_KINDS[kind] ?? kind}.`));
  box.append(heading);
  box.append(make("pre", "", message));
  element("summary").replaceChildren(box);
  element("record-box").hidden = true;
  showLog(log);
}

function showLog(log) {
  const lines = Array.isArray(log) ? log : [];
  element("log").textContent = lines.join("\n");
  element("log-box").hidden = lines.length === 0;
}

readAddress();
