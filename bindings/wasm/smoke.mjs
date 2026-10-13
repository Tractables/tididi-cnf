// Runs the built module on the page's examples, under the task each declares,
// and on the instances bindings/parity.txt lists, under the mode it names, and
// checks each answer against the native binary's on the same file: the mode,
// the count and the counts in the record, and that the value is the count's
// as a number. Then checks a time limit, and that a malformed input, a task
// the header contradicts and an unknown task are errors of the kind the record
// would name.
//
//   node smoke.mjs <bonsai.js> <repository> <tididi>
//
// <repository> is the checkout the module was built from, whose tests/data/
// holds the files examples.js names, and <tididi> the native binary built
// from it. libgmp.so and libgmpxx.so have to be beside bonsai.js; abi.js and
// examples.js beside this file.

import childProcess from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { EXAMPLES } from "./examples.js";

const [modulePath, repository, nativePath] = process.argv.slice(2);
if (!modulePath || !repository || !nativePath) {
  console.error("usage: node smoke.mjs <bonsai.js> <repository> <tididi>");
  process.exit(2);
}
// The module and the ABI load as classic scripts, the way the page's worker
// loads them.
const require = createRequire(import.meta.url);
const createBonsai = require(path.resolve(modulePath));
const { bonsaiAbi } = require(path.join(path.dirname(fileURLToPath(import.meta.url)), "abi.js"));

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
function checkParity(bonsai, name, file, mode, timeLimitMs = 0) {
  const label = `${name}${mode === null ? "" : `, task ${mode}`}`;
  const started = Date.now();
  const module = bonsai.count(fs.readFileSync(file, "utf8"), mode, timeLimitMs);
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

function checkRefused(bonsai, label, dimacs, mode, kind) {
  const answer = bonsai.count(dimacs, mode, 0);
  console.log(`${label}: ${JSON.stringify(answer.error)}`);
  check(!answer.ok && answer.error.kind === kind, `${label}: expected an error of kind ${kind}`);
}

const bonsai = bonsaiAbi(await createBonsai());
const file = (name) => path.join(repository, "tests", "data", name);

for (const [, name, example] of EXAMPLES) checkParity(bonsai, `${example} (${name})`, file(example), null);

// The instances every binding is checked on, each under the mode it names.
const parity = fs.readFileSync(path.join(repository, "bindings", "parity.txt"), "utf8");
for (const line of parity.split("\n")) {
  if (line.trim() === "" || line.startsWith("#")) continue;
  const [instance, mode] = line.trim().split(/\s+/);
  checkParity(bonsai, instance, path.join(repository, instance), mode);
}

// A headerless formula with a time limit.
checkParity(bonsai, "queens6.cnf, a minute to run", file("queens6.cnf"), "mc", 60000);

checkRefused(bonsai, "malformed", "p cnf 2 1\n1 x 0\n", null, "input");
checkRefused(
  bonsai,
  "wmc_header_tiny_sat.cnf under task mc",
  fs.readFileSync(file("wmc_header_tiny_sat.cnf"), "utf8"),
  "mc",
  "usage",
);
checkRefused(bonsai, "unknown task", fs.readFileSync(file("simple.cnf"), "utf8"), "count", "usage");

if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("smoke: all checks passed");
