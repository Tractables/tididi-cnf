// The evaluation's results: the runs datasets.json lists, one CSV per entry,
// read into the sets, the solvers and the instances the page shows, and the
// counts the page derives from them. Nothing here touches the page.

const COLUMNS = "set,instance,solver,solved,seconds";

// The solver this repository builds. The page gives it the first colour and
// draws it on top.
export const OURS = "tididi-cnf";

// Whether a solver printed its count on an instance in time.
export const solvedBy = (instance, solver) => instance.times.get(solver) !== null;

// Reads one CSV of runs: a row per instance and solver, with whether the
// solver printed its count in time and after how many seconds (empty when it
// did not). Sets and solvers keep the order the rows name them in.
export function parseRuns(text, file, timeout) {
  const lines = text.trim().split(/\r?\n/);
  if (lines[0] !== COLUMNS) throw new Error(`${file}: the header is not ${COLUMNS}`);
  const sets = [];
  const solvers = [];
  const byKey = new Map();
  lines.slice(1).forEach((line, index) => {
    const where = `${file}, line ${index + 2}`;
    const fields = line.split(",");
    if (fields.length !== 5) throw new Error(`${where}: ${fields.length} fields instead of 5`);
    const [set, name, solver, solved, seconds] = fields;
    if (solved !== "yes" && solved !== "no") throw new Error(`${where}: solved is "${solved}", not yes or no`);
    if ((solved === "yes") !== (seconds !== "")) throw new Error(`${where}: seconds are given exactly when solved`);
    const time = solved === "yes" ? Number(seconds) : null;
    if (time !== null && !(time >= 0 && time <= timeout)) {
      throw new Error(`${where}: ${seconds} is not a time between 0 and the ${timeout} s limit`);
    }
    if (!sets.includes(set)) sets.push(set);
    if (!solvers.includes(solver)) solvers.push(solver);
    const key = `${set}\n${name}`;
    if (!byKey.has(key)) byKey.set(key, { set, name, times: new Map() });
    const { times } = byKey.get(key);
    if (times.has(solver)) throw new Error(`${where}: a second row for ${solver} on ${name}`);
    times.set(solver, time);
  });
  const instances = [...byKey.values()];
  for (const instance of instances) {
    const missing = solvers.find((solver) => !instance.times.has(solver));
    if (missing !== undefined) throw new Error(`${file}: no row for ${missing} on ${instance.name}`);
  }
  return { sets, solvers, instances };
}

// This file's version stamp (see bindings/wasm/README.md), passed on to the
// results it fetches.
const stamp = new URL(import.meta.url).search;

// Every dataset datasets.json lists, beside `base`, with its runs read.
export async function loadDatasets(base) {
  const fetchOk = async (file) => {
    const response = await fetch(new URL(file + stamp, base));
    if (!response.ok) throw new Error(`${file}: ${response.status} ${response.statusText}`);
    return response;
  };
  const entries = await (await fetchOk("datasets.json")).json();
  return Promise.all(entries.map(async ({ file, title, about, timeout_s: timeout, cactus_min_s: cactusLow, colours }) => {
    if (typeof file !== "string" || typeof title !== "string" || typeof about !== "string" || !(timeout > 0)
        || !(cactusLow > 0 && cactusLow < timeout)) {
      throw new Error("datasets.json: each entry needs a file, a title, an about, a positive timeout_s and cactus_min_s below its timeout");
    }
    const runs = parseRuns(await (await fetchOk(file)).text(), file, timeout);
    for (const solver of runs.solvers) {
      const pair = colours?.[solver];
      if (!Array.isArray(pair) || pair.length !== 2 || !pair.every((c) => /^#[0-9a-f]{6}$/i.test(c))) {
        throw new Error(`${file}: ${solver} needs light and dark colours`);
      }
    }
    return { file, title, about, timeout, cactusLow, colours, ...runs };
  }));
}

// Each solver's solved count on `instances`, and how many of those no other
// solver solved.
export function tally(instances, solvers) {
  return solvers.map((solver) => {
    const solved = instances.filter((instance) => solvedBy(instance, solver));
    const alone = solved.filter((instance) => solvers.every((other) => other === solver || !solvedBy(instance, other)));
    return { solver, solved: solved.length, alone: alone.length };
  });
}

// A solver's times on the instances it solved, fastest first: its curve on
// the cactus plot.
export const sortedTimes = (instances, solver) =>
  instances.map((instance) => instance.times.get(solver)).filter((time) => time !== null).sort((x, y) => x - y);

// The fastest time any solver took on an instance, or null when none solved it.
export function fastest(instance) {
  const times = [...instance.times.values()].filter((time) => time !== null);
  return times.length === 0 ? null : Math.min(...times);
}

// Solver a against solver b: the instances each finished first (`only` of
// them the other did not solve), those both finished in the same time, and
// those neither solved.
export function headToHead(instances, a, b) {
  const counts = { a: 0, aOnly: 0, b: 0, bOnly: 0, tie: 0, neither: 0 };
  for (const instance of instances) {
    const ta = instance.times.get(a);
    const tb = instance.times.get(b);
    if (ta === null && tb === null) counts.neither += 1;
    else if (ta === tb) counts.tie += 1;
    else if (tb === null || (ta !== null && ta < tb)) {
      counts.a += 1;
      if (tb === null) counts.aOnly += 1;
    } else {
      counts.b += 1;
      if (ta === null) counts.bOnly += 1;
    }
  }
  return counts;
}
