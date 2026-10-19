// Runs the built module on the page's examples, under the task each declares,
// and on the instances bindings/parity.txt lists, under the mode it names, and
// checks each answer against the native binary's on the same file: the mode,
// the count and the counts in the record, and that the value is the count's
// as a number. Then checks a time limit, and that a malformed input, a task
// the header contradicts and an unknown task are errors of the kind the record
// would name. Last, draws each example small enough to draw and checks every
// step's diagram, read by the page's own tdd.js, against enumeration: its
// model count, and at the last step the probabilities under evidence.
//
//   node smoke.mjs <tididi_cnf.js> <repository> <tididi>
//
// <repository> is the checkout the module was built from, whose tests/data/
// holds the files examples.js names, and <tididi> the native binary built
// from it. tididi_cnf.js is in the directory site.sh fills, beside libgmp.so,
// libgmpxx.so, tdd.js and vtree.js; abi.js and examples.js are beside this
// file.

import childProcess from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { EXAMPLES } from "./examples.js";

const [modulePath, repository, nativePath] = process.argv.slice(2);
if (!modulePath || !repository || !nativePath) {
  console.error("usage: node smoke.mjs <tididi_cnf.js> <repository> <tididi>");
  process.exit(2);
}
// The module and the ABI load as classic scripts, the way the page's worker
// loads them.
const require = createRequire(import.meta.url);
const createTididiCnf = require(path.resolve(modulePath));
const { tididiCnfAbi } = require(path.join(path.dirname(fileURLToPath(import.meta.url)), "abi.js"));

let failures = 0;
function check(condition, what) {
  if (!condition) {
    console.error(`FAIL: ${what}`);
    failures += 1;
  }
}

// The native binary's answer on a file: the count its answer line prints and
// the record `--stats -` writes on the line after it.
function nativeAnswer(file, args) {
  const stdout = childProcess.execFileSync(nativePath, [...args, "--stats", "-", file], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  const [answerLine, recordLine] = stdout.trim().split("\n");
  return { count: answerLine.slice(answerLine.indexOf(": ") + 2), stats: JSON.parse(recordLine) };
}

// The module's answer on `file` under `mode` is the native binary's under the
// matching `--mode`.
function checkParity(compiler, name, file, mode, timeLimitMs = 0) {
  const label = `${name}${mode === null ? "" : `, task ${mode}`}`;
  const started = Date.now();
  const module = compiler.count(fs.readFileSync(file, "utf8"), mode, timeLimitMs);
  check(module.ok, `${label}: ${JSON.stringify(module.error)}`);
  if (!module.ok) return;
  const native = nativeAnswer(file, mode === null ? [] : ["--mode", mode]);
  console.log(`${label}: ${module.mode} ${module.count} in ${Date.now() - started} ms`);
  check(module.count === native.count, `${label}: the module counts ${module.count}, the binary ${native.count}`);
  check(module.mode === native.stats.mode, `${label}: the module ran ${module.mode}, the binary ${native.stats.mode}`);
  check(module.stats.mode === module.mode, `${label}: the record says mode ${module.stats.mode}`);
  check(module.satisfiable === native.stats.satisfiable, `${label}: the verdicts differ`);
  // The value is the count's nearest double: the count read as a number.
  const [numerator, denominator = "1"] = module.count.split("/");
  const reading = Number(numerator) / Number(denominator);
  check(
    Math.abs(module.value - reading) <= 1e-12 * Math.abs(reading),
    `${label}: the value ${module.value} is not the count ${module.count}`,
  );
  for (const key of ["model_count", "weighted_count"]) {
    check(
      module.stats[key] === native.stats[key],
      `${label}: the record's ${key} is ${module.stats[key]}, the binary's ${native.stats[key]}`,
    );
  }
}

function checkRefused(compiler, label, dimacs, mode, kind) {
  const answer = compiler.count(dimacs, mode, 0);
  console.log(`${label}: ${JSON.stringify(answer.error)}`);
  check(!answer.ok && answer.error.kind === kind, `${label}: expected an error of kind ${kind}`);
}

const compiler = tididiCnfAbi(await createTididiCnf());
const file = (name) => path.join(repository, "tests", "data", name);

for (const [, name, example] of EXAMPLES) checkParity(compiler, `${example} (${name})`, file(example), null);

// The instances every binding is checked on, each under the mode it names.
const parity = fs.readFileSync(path.join(repository, "bindings", "parity.txt"), "utf8");
for (const line of parity.split("\n")) {
  if (line.trim() === "" || line.startsWith("#")) continue;
  const [instance, mode] = line.trim().split(/\s+/);
  checkParity(compiler, instance, path.join(repository, instance), mode);
}

// A headerless formula with a time limit.
checkParity(compiler, "queens6.cnf, a minute to run", file("queens6.cnf"), "mc", 60000);

checkRefused(compiler, "malformed", "p cnf 2 1\n1 x 0\n", null, "input");
checkRefused(
  compiler,
  "wmc_header_tiny_sat.cnf under task mc",
  fs.readFileSync(file("wmc_header_tiny_sat.cnf"), "utf8"),
  "mc",
  "usage",
);
checkRefused(compiler, "unknown task", fs.readFileSync(file("simple.cnf"), "utf8"), "count", "usage");

// The page's readers, from the site beside the module.
const site = path.dirname(path.resolve(modulePath));
const { parseVtree } = await import(pathToFileURL(path.join(site, "vtree.js")));
const { diagramValue, leafValues, marginals, nodeAt, nodeValues, parseTdd, readWeights } = await import(
  pathToFileURL(path.join(site, "tdd.js"))
);

// The models of `clauses` over `variables` variables, each as a bit mask.
function enumerate(clauses, variables) {
  const models = [];
  for (let assignment = 0; assignment < 2 ** variables; assignment++) {
    const holds = (literal) => ((assignment >> (Math.abs(literal) - 1)) & 1) === (literal > 0 ? 1 : 0);
    if (clauses.every((clause) => clause.some(holds))) models.push(assignment);
  }
  return models;
}

// The weight of `models` under `weights` and `evidence`, as tdd.js reads them.
function weigh(models, variables, weights, evidence) {
  let total = 0;
  for (const model of models) {
    let weight = 1;
    for (let v = 1; v <= variables; v++) {
      const value = (model >> (v - 1)) & 1;
      if (evidence[v] !== 0 && evidence[v] !== (value === 1 ? 1 : -1)) weight = 0;
      weight *= weights[v][value];
    }
    total += weight;
  }
  return total;
}

const close = (a, b) => Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(b));

function checkDrawing(label, dimacs) {
  const drawing = compiler.draw(dimacs);
  check(drawing.ok, `${label}: ${JSON.stringify(drawing.error)}`);
  if (!drawing.ok) return;
  const tree = parseVtree(drawing.vtree);
  check(tree.error === undefined, `${label}: the vtree: ${tree.error}`);
  if (tree.error !== undefined) return;
  const variables = tree.maxVariable;
  const counting = () => [2, 1, 1];
  let last = null;
  for (const { node: t, tdd } of drawing.steps) {
    const diagram = parseTdd(tdd, tree);
    check(diagram.error === undefined, `${label}, step at node ${t}: ${diagram.error}`);
    if (diagram.error !== undefined) return;
    // The clauses whose variables all lie under t.
    const under = new Set();
    const stack = [t];
    while (stack.length > 0) {
      const u = stack.pop();
      if (tree.left[u] < 0) under.add(tree.variable[u]);
      else stack.push(tree.left[u], tree.right[u]);
    }
    const clauses = drawing.clauses.filter((clause) => clause.every((literal) => under.has(Math.abs(literal))));
    const models = enumerate(clauses, variables).length;
    const counted = diagramValue(diagram, tree, nodeValues(diagram, tree, counting));
    check(counted === models, `${label}, step at node ${t}: the diagram counts ${counted}, enumeration ${models}`);
    // The page finds the step's own node this way; it throws when it cannot.
    nodeAt(diagram, tree, t);
    if (t === tree.root) last = diagram;
  }
  check(drawing.steps.length === variables - 1, `${label}: ${drawing.steps.length} steps for ${variables} variables`);
  const models = enumerate(drawing.clauses, variables);
  const weights = readWeights(drawing.weights, variables);
  const pinned = (pins) => {
    const evidence = new Int8Array(variables + 1);
    for (const [v, pin] of pins) evidence[v] = pin;
    return evidence;
  };
  for (const evidence of [pinned([]), pinned([[1, 1]]), pinned([[variables, -1], [2, 1]])]) {
    const { value, shares } = marginals(last, tree, weights, evidence);
    const want = weigh(models, variables, weights, evidence);
    check(close(value, want), `${label}, evidence ${evidence}: the diagram weighs ${value}, enumeration ${want}`);
    if (want === 0) continue;
    for (let v = 1; v <= variables; v++) {
      if (evidence[v] !== 0) continue;
      const share = weigh(models, variables, weights, Int8Array.from(evidence, (pin, u) => (u === v ? 1 : pin))) / want;
      check(close(shares[v], share), `${label}, evidence ${evidence}: x${v} has share ${shares[v]}, enumeration ${share}`);
    }
  }
  console.log(`${label}: drawn in ${drawing.steps.length} steps`);
}

for (const [, name, example] of EXAMPLES) {
  const dimacs = fs.readFileSync(file(example), "utf8");
  const drawing = compiler.draw(dimacs);
  if (!drawing.ok && drawing.error.kind === "size") continue;
  checkDrawing(`${example} (${name}), drawn`, dimacs);
}
checkDrawing("unsat_empty_clause.cnf, drawn", fs.readFileSync(file("unsat_empty_clause.cnf"), "utf8"));
const unwieldy = compiler.draw(fs.readFileSync(file("queens6.cnf"), "utf8"));
console.log(`queens6.cnf, drawn: ${JSON.stringify(unwieldy.error)}`);
check(!unwieldy.ok && unwieldy.error.kind === "size", "queens6.cnf, drawn: expected an error of kind size");

if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("smoke: all checks passed");
