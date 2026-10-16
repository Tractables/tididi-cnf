// The evaluation page: the choice of runs and of instances, the table of
// solved counts, the cactus plot, the head-to-head plot and the table of every
// instance, all drawn from the datasets results.js reads.

import { drawCactus, drawScatter } from "./charts.js";
import { OURS, fastest, headToHead, loadDatasets, solvedBy, sortedTimes, tally } from "./results.js";

const element = (id) => document.getElementById(id);
const make = (name, className, text) => {
  const node = document.createElement(name);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};
const setOptions = (select, entries) =>
  select.replaceChildren(...entries.map(([value, text]) => {
    const option = make("option", "", text);
    option.value = value;
    return option;
  }));

const control = {
  dataset: element("dataset"),
  set: element("set"),
  a: element("pair-a"),
  b: element("pair-b"),
  search: element("search"),
  solved: element("solved-by"),
  unsolved: element("unsolved-by"),
};
const cactus = element("cactus");
const pair = element("pair");

let datasets = [];
// Every solver of every dataset, ours first: a solver's colour is its place
// here, so it keeps its colour from one dataset to the next.
let palette = [];
let dataset = null;
// The solvers switched off on the cactus plot.
const hidden = new Set();
// The instance table's order: the column, and 1 or -1 for up or down.
let order = { key: "instance", direction: 1 };
// The two solvers on the head-to-head plot.
let pairShown = [];

const colourOf = (solver) => `var(--series-${palette.indexOf(solver)}, var(--muted))`;
const key = (colour) => {
  const swatch = make("i", "key");
  swatch.style.background = colour;
  return swatch;
};
// The width each plot's box had when the plot was drawn.
const drawnAt = new Map();
const drawnWidth = (svg) => {
  const box = svg.parentElement;
  drawnAt.set(box, box.clientWidth);
  return box.clientWidth;
};
const inScope = () =>
  control.set.value === "" ? dataset.instances : dataset.instances.filter((instance) => instance.set === control.set.value);
const scopeName = () => control.set.value || "all sets";

// ------------------------------------------------------------- choosing

// Fills the controls that depend on the dataset, keeping each choice the new
// dataset also offers.
function chooseDataset() {
  dataset = datasets[Number(control.dataset.value)];
  const { sets, solvers, instances } = dataset;
  const keep = (select, entries, fallback) => {
    const previous = select.value;
    setOptions(select, entries);
    select.value = entries.some(([value]) => value === previous) ? previous : fallback;
  };
  const size = (set) => instances.filter((instance) => instance.set === set).length;
  const scopes = sets.map((set) => [set, `${set} (${size(set)})`]);
  if (sets.length > 1) scopes.unshift(["", `All sets (${instances.length})`]);
  keep(control.set, scopes, scopes[0][0]);
  const named = solvers.map((solver) => [solver, solver]);
  keep(control.solved, [["", "no filter"], ...named], "");
  keep(control.unsolved, [["", "no filter"], ...named], "");
  keep(control.b, named, solvers.includes(OURS) ? OURS : solvers[0]);
  const other = solvers.find((solver) => solver !== control.b.value);
  keep(control.a, named, other);
  if (control.a.value === control.b.value) control.a.value = other;
  element("cactus-solvers").replaceChildren(...solvers.map((solver) => {
    const toggle = make("button", "toggle");
    toggle.type = "button";
    toggle.dataset.solver = solver;
    toggle.append(key(colourOf(solver)), solver, make("span", "count"));
    toggle.onclick = () => {
      if (!hidden.delete(solver)) hidden.add(solver);
      renderCactus();
    };
    return toggle;
  }));
  element("pair-card").hidden = solvers.length < 2;
  const link = make("a", "", dataset.file);
  link.href = dataset.file;
  element("dataset-note").replaceChildren(`${dataset.timeout} s per instance. The result of every run: `, link, ".");
  render();
}

// ------------------------------------------------------------- drawing

function render() {
  renderSummary();
  renderCactus();
  renderPair();
  renderInstances();
}

// Solved counts per set and in all, and how many no other solver solved, the
// best count of each set in bold and the set on show marked.
function renderSummary() {
  const { sets, solvers, instances } = dataset;
  const columns = sets.map((set) => ({ set, instances: instances.filter((instance) => instance.set === set) }));
  if (sets.length > 1) columns.push({ set: "", label: "All", instances });
  const tallies = columns.map((column) => tally(column.instances, solvers));
  const cell = (name, text, column, extra = "") => {
    const node = make(name, `${column.set === control.set.value ? "picked" : ""} ${extra}`.trim(), text);
    if (name === "th") node.scope = "col";
    return node;
  };

  const groups = make("tr");
  const corner = make("th", "", "Solver");
  corner.rowSpan = 2;
  corner.scope = "col";
  groups.append(corner);
  for (const title of ["Solved", "Solved by no other"]) {
    const group = make("th", "group", title);
    group.colSpan = columns.length;
    group.scope = "colgroup";
    groups.append(group);
  }
  const names = make("tr");
  for (const solvedColumn of [true, false]) {
    for (const column of columns) {
      const label = column.label ?? column.set;
      names.append(cell("th", solvedColumn ? `${label} (${column.instances.length})` : label, column));
    }
  }

  const body = make("tbody");
  solvers.forEach((solver, row) => {
    const line = make("tr");
    const name = make("th", "", "");
    name.scope = "row";
    name.append(key(colourOf(solver)), solver);
    line.append(name);
    for (const field of ["solved", "alone"]) {
      tallies.forEach((counts, k) => {
        const value = counts[row][field];
        const best = field === "solved" && value === Math.max(...counts.map((c) => c.solved));
        line.append(cell("td", String(value), columns[k], best ? "best" : ""));
      });
    }
    body.append(line);
  });
  const head = make("thead");
  head.append(groups, names);
  element("summary").replaceChildren(head, body);
  element("summary-note").textContent =
    `Instances solved in ${dataset.timeout} s each, the most of each set in bold. Solved by no other: of those, the ones no other solver here solved.`;
}

function renderCactus() {
  const instances = inScope();
  const curves = dataset.solvers.map((solver) => ({
    name: solver,
    colour: colourOf(solver),
    ours: solver === OURS,
    times: sortedTimes(instances, solver),
  }));
  for (const toggle of element("cactus-solvers").children) {
    toggle.setAttribute("aria-pressed", String(!hidden.has(toggle.dataset.solver)));
    toggle.lastChild.textContent = String(curves.find((curve) => curve.name === toggle.dataset.solver).times.length);
  }
  drawCactus(cactus, {
    width: drawnWidth(cactus),
    curves: curves.filter((curve) => !hidden.has(curve.name)),
    most: Math.max(1, ...curves.map((curve) => curve.times.length)),
    total: instances.length,
    low: dataset.low,
    timeout: dataset.timeout,
    title: `Instances of ${scopeName()} solved within a time limit, up to ${dataset.timeout} s`,
  });
}

function renderPair() {
  if (dataset.solvers.length < 2) return;
  const [a, b] = [control.a.value, control.b.value];
  pairShown = [a, b];
  const counts = headToHead(inScope(), a, b);
  const tie = "var(--muted)";
  const item = (colour, text) => {
    const line = make("li");
    line.append(key(colour), text);
    return line;
  };
  element("pair-counts").replaceChildren(
    item(colourOf(a), `${a} first on ${counts.a}, ${counts.aOnly} of them not solved by ${b}`),
    item(colourOf(b), `${b} first on ${counts.b}, ${counts.bOnly} of them not solved by ${a}`),
    item(tie, `The same time on ${counts.tie}; neither solves ${counts.neither}.`),
  );
  drawScatter(pair, {
    width: drawnWidth(pair),
    a: { name: a, colour: colourOf(a) },
    b: { name: b, colour: colourOf(b) },
    instances: inScope(),
    low: dataset.low,
    timeout: dataset.timeout,
    tie,
    title: `${a} against ${b} on each instance of ${scopeName()}`,
  });
}

// The instance table: the instances in scope that pass the filters, in the
// chosen order, an instance a solver did not solve last whichever way its
// column runs, and the fastest time of each instance marked.
function renderInstances() {
  const { sets, solvers } = dataset;
  const showSet = control.set.value === "" && sets.length > 1;
  const columns = [["instance", "Instance"], ...(showSet ? [["set", "Set"]] : []), ...solvers.map((solver) => [solver, solver])];
  if (!columns.some(([column]) => column === order.key)) order = { key: "instance", direction: 1 };

  const byName = (x, y) => x.name.localeCompare(y.name, "en", { numeric: true });
  const compare = (x, y) => {
    if (order.key === "instance") return order.direction * byName(x, y);
    if (order.key === "set") return order.direction * (sets.indexOf(x.set) - sets.indexOf(y.set)) || byName(x, y);
    const [tx, ty] = [x.times.get(order.key), y.times.get(order.key)];
    if ((tx === null) !== (ty === null)) return tx === null ? 1 : -1;
    return order.direction * (tx - ty) || byName(x, y);
  };
  const text = control.search.value.trim().toLowerCase();
  const instances = inScope();
  const rows = instances
    .filter((instance) => !text || instance.name.toLowerCase().includes(text))
    .filter((instance) => !control.solved.value || solvedBy(instance, control.solved.value))
    .filter((instance) => !control.unsolved.value || !solvedBy(instance, control.unsolved.value))
    .sort(compare);

  const head = make("tr");
  for (const [column, label] of columns) {
    const th = make("th", column === "set" ? "set" : "");
    th.scope = "col";
    if (column === order.key) th.setAttribute("aria-sort", order.direction > 0 ? "ascending" : "descending");
    const button = make("button", "sort");
    button.type = "button";
    if (solvers.includes(column)) button.append(key(colourOf(column)));
    button.append(label, make("span", "arrow", column === order.key ? (order.direction > 0 ? "▲" : "▼") : ""));
    button.onclick = () => {
      order = { key: column, direction: column === order.key ? -order.direction : 1 };
      renderInstances();
      element("instances").querySelector("[aria-sort] button").focus();
    };
    th.append(button);
    head.append(th);
  }

  const body = make("tbody");
  for (const instance of rows) {
    const line = make("tr");
    const name = make("th", "", instance.name);
    name.scope = "row";
    line.append(name);
    if (showSet) line.append(make("td", "set", instance.set));
    const best = fastest(instance);
    for (const solver of solvers) {
      const time = instance.times.get(solver);
      const td = make("td", time === null ? "none" : time === best ? "fastest" : "");
      if (time === null) {
        td.append(make("span", "", "–"), make("span", "visually-hidden", "not solved"));
        td.firstChild.setAttribute("aria-hidden", "true");
      } else {
        td.textContent = time.toFixed(2);
      }
      line.append(td);
    }
    body.append(line);
  }
  const thead = make("thead");
  thead.append(head);
  element("instances").replaceChildren(thead, body);
  element("instance-count").textContent = `${rows.length} of ${instances.length} instances; times in seconds, the fastest marked.`;
}

// ------------------------------------------------------------- wiring

control.dataset.onchange = chooseDataset;
control.set.onchange = render;
// The two solvers of the head-to-head stay distinct: picking the other one's
// solver swaps them.
control.a.onchange = () => {
  if (control.a.value === control.b.value) control.b.value = pairShown[0];
  renderPair();
};
control.b.onchange = () => {
  if (control.b.value === control.a.value) control.a.value = pairShown[1];
  renderPair();
};
for (const filter of [control.search, control.solved, control.unsolved]) filter.oninput = renderInstances;

// Each plot is drawn at the width of its box, and again when that changes.
const resized = new ResizeObserver((entries) => {
  if (dataset === null) return;
  for (const { target } of entries) {
    if (drawnAt.get(target) === target.clientWidth) continue;
    if (target === cactus.parentElement) renderCactus();
    else renderPair();
  }
});
resized.observe(cactus.parentElement);
resized.observe(pair.parentElement);

try {
  datasets = await loadDatasets(import.meta.url);
  palette = [...new Set([OURS, ...datasets.flatMap((d) => d.solvers)])];
  for (const d of datasets) {
    // The time axes start at the power of ten at or below the fastest run.
    const times = d.instances.flatMap((instance) => [...instance.times.values()]).filter((time) => time > 0);
    d.low = 10 ** Math.floor(Math.log10(times.length === 0 ? d.timeout / 1000 : Math.min(...times)));
  }
  setOptions(control.dataset, datasets.map((d, i) => [String(i), d.title]));
  chooseDataset();
} catch (error) {
  const shown = element("load-error");
  shown.textContent = `The results did not load: ${error.message}`;
  shown.hidden = false;
}
