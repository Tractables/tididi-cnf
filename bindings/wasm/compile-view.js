// The card that shows how a small formula is compiled and what its diagram
// answers: the compile's steps over the vtree, the clauses as they join it,
// and each variable's probability under the evidence pinned on the page. Its
// input is what the module's draw call returns (abi.js).

import { isLeaf, parseVtree } from "./vtree.js";
import { layout, prepareStep, render } from "./picture.js";
import { diagramValue, leafValues, marginals, nodeAt, nodeValues, parseTdd, readWeights } from "./tdd.js";
import { plural } from "./words.js";

// The pause between steps while the compile plays.
const PLAY_MS = 1100;

const element = (id) => document.getElementById(id);
const make = (name, className, text) => {
  const node = document.createElement(name);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const card = element("compile-card");
const picture = element("picture");
const slider = element("step");
const playButton = element("step-play");

// What is on show: the vtree, the steps in the order the compile takes them,
// the layout, the clauses and where each waits, the weights, the evidence and
// the step on show. null when nothing is.
let shown = null;
let playing = 0;

// ------------------------------------------------------------ the drawing

// Shows the drawing `drawing`, or why there is none, or, for null, nothing.
export function showDrawing(drawing) {
  stop();
  element("compile-stale").hidden = true;
  card.hidden = drawing === null;
  if (drawing === null) return;
  shown = null;
  if (drawing.ok !== true) {
    noDrawing(`No drawing: ${drawing.error.message}.`);
    return;
  }
  try {
    shown = read(drawing);
  } catch (failure) {
    noDrawing(`The drawing did not load: ${failure.message}`);
    return;
  }
  element("compile-unavailable").hidden = true;
  element("compile-body").hidden = false;
  slider.max = String(shown.order.length);
  listClauses();
  buildAsk();
  go(shown.order.length);
}

export function markDrawingStale() {
  if (!card.hidden) element("compile-stale").hidden = false;
}

function noDrawing(message) {
  element("compile-unavailable").textContent = message;
  element("compile-unavailable").hidden = false;
  element("compile-body").hidden = true;
}

function read(drawing) {
  const tree = parseVtree(drawing.vtree);
  if (tree.error !== undefined) throw new Error(`its vtree: ${tree.error}`);
  const texts = new Map(drawing.steps.map(({ node, tdd }) => [node, tdd]));

  // The internal nodes, each after its subtree, the left one first: the
  // order the steps are shown in.
  const order = [];
  const visit = (t) => {
    if (isLeaf(tree, t)) return;
    visit(tree.left[t]);
    visit(tree.right[t]);
    order.push(t);
  };
  visit(tree.root);
  const stepOf = new Int32Array(tree.count);
  order.forEach((t, k) => {
    stepOf[t] = k + 1;
  });

  const steps = order.map((t) => {
    if (!texts.has(t)) throw new Error(`there is no diagram for vtree node ${t}`);
    const diagram = parseTdd(texts.get(t), tree);
    if (diagram.error !== undefined) throw new Error(`the diagram at vtree node ${t}: ${diagram.error}`);
    const counts = nodeValues(diagram, tree, () => [2, 1, 1]);
    return prepareStep(tree, t, diagram, counts, nodeAt(diagram, tree, t));
  });

  // The variables under each vtree node, one bit each (a drawing has at most
  // 16 variables); where each clause waits, the lowest internal node whose
  // subtree holds its variables; and how many clauses wait at each node.
  const under = new Array(tree.count).fill(0);
  for (let i = tree.count - 1; i >= 0; i--) {
    const t = tree.order[i];
    under[t] = isLeaf(tree, t) ? 1 << (tree.variable[t] - 1) : under[tree.left[t]] | under[tree.right[t]];
  }
  const holds = (t, mask) => !isLeaf(tree, t) && (under[t] & mask) === mask;
  const clauses = drawing.clauses.map((literals) => {
    const mask = literals.reduce((bits, literal) => bits | (1 << (Math.abs(literal) - 1)), 0);
    let t = tree.root;
    for (;;) {
      const below = [tree.left[t], tree.right[t]].find((child) => holds(child, mask));
      if (below === undefined) break;
      t = below;
    }
    return { literals, node: t };
  });
  const waiting = new Int32Array(tree.count);
  for (const clause of clauses) waiting[clause.node] += 1;

  return {
    tree,
    order,
    stepOf,
    steps,
    place: layout(tree, steps, waiting),
    under,
    clauses,
    waiting,
    weighted: drawing.weights !== null,
    weights: readWeights(drawing.weights, tree.maxVariable),
    evidence: new Int8Array(tree.maxVariable + 1),
    step: 0,
  };
}

const variablesUnder = (t) => {
  const names = [];
  for (let v = 1; v <= shown.tree.maxVariable; v++) if (shown.under[t] & (1 << (v - 1))) names.push(`x${v}`);
  return names;
};

const literalText = (literal) => (literal > 0 ? `x${literal}` : `¬x${-literal}`);

// A node value as the picture and the answers write it: a count in full, a
// weighted value to four significant digits.
function format(value) {
  if (Number.isInteger(value) && Math.abs(value) < 1e15) return value.toLocaleString("en-US");
  return String(Number(value.toPrecision(4)));
}

// ----------------------------------------------------------------- steps

// Shows step `k`: 0 before the compile, and k after the k-th internal node.
function go(k) {
  shown.step = Math.max(0, Math.min(k, shown.order.length));
  slider.value = String(shown.step);
  element("step-first").disabled = element("step-back").disabled = shown.step === 0;
  element("step-next").disabled = element("step-last").disabled = shown.step === shown.order.length;
  for (const item of element("clause-list").children) {
    const placed = shown.stepOf[shown.clauses[Number(item.dataset.clause)].node];
    item.className = placed < shown.step ? "in" : placed === shown.step ? "joining" : "waiting";
  }
  element("step-text").textContent = stepText();
  draw(true);
}

function stepText() {
  const { step, order, steps, tree } = shown;
  if (step === 0) {
    return "Before the compile. Each clause waits at the lowest vtree node whose subtree holds all its variables; the number beside a node says how many wait there.";
  }
  const t = order[step - 1];
  const current = steps[step - 1];
  const variables = variablesUnder(t);
  const joining = shown.clauses.filter((clause) => clause.node === t).length;
  const over =
    t === tree.root
      ? `the root, over all ${variables.length} variables`
      : variables.length <= 6
        ? `the node over ${variables.join(", ")}`
        : `a node over ${variables.length} variables`;
  const parts = [`Step ${step} of ${order.length}, ${over}.`];
  parts.push(
    joining === 0
      ? "Its two sides are joined; no clause waits here."
      : `Its two sides are joined, then the ${plural(joining, "clause")} waiting here ${joining === 1 ? "is" : "are"} conjoined in.`,
  );
  if (current.node === null) {
    parts.push("No assignment satisfies the clauses so far: the diagram is false, and the formula has no models.");
  } else {
    const models = current.counts[t][current.node];
    const assignments = 2 ** variables.length;
    let nodes = 0;
    let pairs = 0;
    for (const [u, row] of current.rows) {
      nodes += row.length;
      for (const i of row) pairs += current.diagram.levels[u][i].length / 2;
    }
    parts.push(
      nodes === 0
        ? `Every one of the ${format(assignments)} assignments to these variables satisfies it.`
        : `Its diagram draws ${plural(nodes, "node")} and ${plural(pairs, "pair")}; ${format(models)} of the ${format(assignments)} assignments to these variables satisfy it.`,
    );
  }
  if (step === order.length) parts.push("This is the formula's diagram.");
  return parts.join(" ");
}

// Draws the step on show, `fresh` when it has just been reached, which fades
// its diagram in.
function draw(fresh) {
  const { tree, order, stepOf, steps, step, waiting, weights, evidence } = shown;
  const done = (t) => !isLeaf(tree, t) && stepOf[t] <= step;
  const pieces = [];
  for (let k = 0; k < step; k++) {
    const t = order[k];
    if (tree.parent[t] >= 0 && done(tree.parent[t])) continue;
    const piece = steps[k];
    const values = nodeValues(piece.diagram, tree, leafValues(weights, evidence));
    pieces.push({ step: piece, values, fresh: fresh && k === step - 1 });
  }
  const clauses = Array.from(waiting, (count, t) => ({ count, placed: done(t) }));
  const zoom = render(picture, tree, shown.place, { pieces, clauses, evidence, format });
  if (fresh && step > 0) {
    // Bring the node just compiled into view in a picture that scrolls.
    const { x, y } = shown.place;
    const box = picture.parentElement;
    const t = order[step - 1];
    box.scrollTo({ left: x[t] * zoom - box.clientWidth / 2, top: y[t] * zoom - box.clientHeight / 3, behavior: "smooth" });
  }
}

// A new width can call for another zoom.
let resizing = 0;
addEventListener("resize", () => {
  clearTimeout(resizing);
  resizing = setTimeout(() => {
    if (shown !== null && !card.hidden) draw(false);
  }, 150);
});

function stop() {
  clearInterval(playing);
  playing = 0;
  playButton.textContent = "Play";
}

element("step-first").addEventListener("click", () => {
  stop();
  go(0);
});
element("step-back").addEventListener("click", () => {
  stop();
  go(shown.step - 1);
});
element("step-next").addEventListener("click", () => {
  stop();
  go(shown.step + 1);
});
element("step-last").addEventListener("click", () => {
  stop();
  go(shown.order.length);
});
slider.addEventListener("input", () => {
  stop();
  go(Number(slider.value));
});
playButton.addEventListener("click", () => {
  if (playing !== 0) {
    stop();
    return;
  }
  if (shown.step === shown.order.length) go(0);
  playButton.textContent = "Pause";
  playing = setInterval(() => {
    go(shown.step + 1);
    if (shown.step === shown.order.length) stop();
  }, PLAY_MS);
});

// --------------------------------------------------------------- clauses

function listClauses() {
  const list = element("clause-list");
  list.replaceChildren(
    ...shown.clauses.map((clause, i) => {
      const item = make("li", "", clause.literals.length === 0 ? "⊥ (the empty clause)" : clause.literals.map(literalText).join(" ∨ "));
      item.dataset.clause = String(i);
      return item;
    }),
  );
}

// Pointing at a clause lights the node it waits at and its variables' leaves;
// pointing at a node's count lights the clauses waiting there.
function light(node, variables, clauses) {
  for (const lit of card.querySelectorAll(".lit")) lit.classList.remove("lit");
  if (node !== null) picture.querySelector(`[data-node="${node}"]`)?.classList.add("lit");
  for (const v of variables) picture.querySelector(`[data-variable="${v}"]`)?.classList.add("lit");
  for (const i of clauses) element("clause-list").children[i].classList.add("lit");
}

element("clause-list").addEventListener("pointerover", (event) => {
  const item = event.target.closest("li");
  if (item === null) return;
  const i = Number(item.dataset.clause);
  const clause = shown.clauses[i];
  light(clause.node, clause.literals.map(Math.abs), [i]);
});
picture.addEventListener("pointerover", (event) => {
  const badge = event.target.closest("[data-node]");
  if (badge === null) return;
  const node = Number(badge.dataset.node);
  const waiting = shown.clauses.flatMap((clause, i) => (clause.node === node ? [i] : []));
  light(node, [], waiting);
});
for (const area of [element("clause-list"), picture]) area.addEventListener("pointerleave", () => light(null, [], []));

// ------------------------------------------------------------------- ask

const PINS = [
  [0, "either"],
  [1, "true"],
  [-1, "false"],
];

function buildAsk() {
  const { tree, weighted } = shown;
  element("ask-text").textContent = weighted
    ? "Pin variables true or false, here or by clicking a leaf. The diagram is evaluated once per bar, bottom-up: a pair multiplies the weights of its two sides and a node adds its pairs. Each bar is the variable's share of the weight of the models that agree with the evidence: its probability given the evidence, when the weights are probabilities."
    : "Pin variables true or false, here or by clicking a leaf. The diagram is evaluated once per bar, bottom-up: a pair multiplies the counts of its two sides and a node adds its pairs. Each bar is the share of the models that agree with the evidence and set the variable true.";
  const rows = [];
  for (let v = 1; v <= tree.maxVariable; v++) {
    const row = make("div", "ask-row");
    row.dataset.variable = String(v);
    const pins = make("span", "pins");
    pins.setAttribute("role", "group");
    pins.setAttribute("aria-label", `x${v}`);
    for (const [pin, word] of PINS) {
      const button = make("button", "", word);
      button.type = "button";
      button.dataset.pin = String(pin);
      pins.append(button);
    }
    const bar = make("span", "bar");
    bar.append(make("span", "fill"));
    row.append(make("span", "variable", `x${v}`), pins, bar, make("span", "share"));
    rows.push(row);
  }
  element("evidence").replaceChildren(...rows);
  answer();
}

function answer() {
  const { tree, steps, weights, evidence, weighted } = shown;
  const formula = steps[steps.length - 1].diagram;
  const free = new Int8Array(evidence.length);
  const pinned = evidence.some((pin) => pin !== 0);
  const { value, shares } = marginals(formula, tree, weights, evidence);
  const total = diagramValue(formula, tree, nodeValues(formula, tree, leafValues(weights, free)));
  let line;
  if (total === 0) line = "The formula has no models, so every bar is empty.";
  else if (value === 0) line = "No model agrees with the evidence.";
  else if (weighted) line = pinned ? `The models that agree with the evidence weigh ${format(value)} of ${format(total)}.` : `The models weigh ${format(value)} in all.`;
  else line = pinned ? `${format(value)} of the ${format(total)} models agree with the evidence.` : `${format(value)} of the ${format(2 ** tree.maxVariable)} assignments are models.`;
  element("ask-total").textContent = line;
  element("evidence-clear").disabled = !pinned;
  for (const row of element("evidence").children) {
    const v = Number(row.dataset.variable);
    for (const button of row.querySelectorAll("button")) {
      button.setAttribute("aria-pressed", String(Number(button.dataset.pin) === evidence[v]));
    }
    const share = shares[v];
    row.querySelector(".fill").style.width = Number.isNaN(share) ? "0" : `${(100 * Math.min(1, Math.max(0, share))).toFixed(1)}%`;
    row.querySelector(".share").textContent = Number.isNaN(share) ? "–" : `${(100 * share).toFixed(1)}%`;
  }
}

function pin(v, value) {
  shown.evidence[v] = value;
  answer();
  draw(false);
}

element("evidence").addEventListener("click", (event) => {
  const button = event.target.closest("button");
  if (button !== null) pin(Number(button.closest(".ask-row").dataset.variable), Number(button.dataset.pin));
});
picture.addEventListener("click", (event) => {
  const leaf = event.target.closest("[data-variable]");
  if (leaf === null) return;
  const v = Number(leaf.dataset.variable);
  // Either, then true, then false.
  pin(v, shown.evidence[v] === 0 ? 1 : shown.evidence[v] > 0 ? -1 : 0);
});
element("evidence-clear").addEventListener("click", () => {
  shown.evidence.fill(0);
  answer();
  draw(false);
});
