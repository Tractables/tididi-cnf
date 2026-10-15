// The picture of a compile: the vtree, drawn faintly, and over it the
// diagrams built so far, each node placed at the vtree node its level is. The
// layout is computed once for every step, so a vtree node stays where it is
// from one step to the next.

import { isLeaf } from "./vtree.js";
import { NEGATIVE, POSITIVE, TRUE } from "./tdd.js";

const SVG = "http://www.w3.org/2000/svg";

// Sizes, in pixels.
const ROW = 92;
const NODE_RADIUS = 8;
const PAIR_DROP = 24;
const CELL_WIDTH = 28;
const CELL_HEIGHT = 18;
const PAIR_WIDTH = 2 * CELL_WIDTH;
const PAIR_GAP = 4;
const NODE_GAP = 12;
const LEAF_WIDTH = 34;
const SIBLING_GAP = 10;
const MARGIN = 24;
// The most a small picture is enlarged to fill the width it has, and the
// least a large one is shrunk before it scrolls instead.
const MOST_ZOOM = 1.8;
const LEAST_ZOOM = 0.75;
// Room left of a level's row for the count of clauses waiting there.
const BADGE_ROOM = 28;

// A step's diagram, ready to draw: the vtree node `top` it was compiled at,
// the diagram, its top node (null when the diagram is false), the number of
// models of every node, and the nodes a picture draws, level by level.
export function prepareStep(tree, top, diagram, counts, node) {
  const step = { top, diagram, counts, node, rows: new Map() };
  if (node === null || isFree(tree, step, top, node)) return step;
  const seen = new Set();
  const visit = (t, i) => {
    if (seen.has(`${t} ${i}`)) return;
    seen.add(`${t} ${i}`);
    if (!step.rows.has(t)) step.rows.set(t, []);
    step.rows.get(t).push(i);
    const pairs = diagram.levels[t][i];
    for (let k = 0; k < pairs.length; k++) {
      const child = k % 2 === 0 ? tree.left[t] : tree.right[t];
      if (!isLeaf(tree, child) && !isFree(tree, step, child, pairs[k])) visit(child, pairs[k]);
    }
  };
  visit(top, node);
  return step;
}

// Whether node `i` of level `t` holds every assignment to the variables under
// `t`. The picture writes such a node as ⊤ rather than drawing it.
export const isFree = (tree, step, t, i) => step.counts[t][i] === 2 ** tree.leaves[t];

const nodeWidth = (pairs) => Math.max(2 * NODE_RADIUS, (pairs / 2) * (PAIR_WIDTH + PAIR_GAP) - PAIR_GAP);

function rowWidth(step, t) {
  const nodes = step.rows.get(t);
  let width = (nodes.length - 1) * NODE_GAP;
  for (const i of nodes) width += nodeWidth(step.diagram.levels[t][i].length);
  return width;
}

// Where everything goes: each vtree node gets a span wide enough for its two
// children's and for the widest row any step draws at its level, with room
// before it where `waiting[t]` clauses wait at the node, and sits over the
// middle of its children.
export function layout(tree, steps, waiting) {
  const own = new Float64Array(tree.count);
  const lead = Float64Array.from(waiting, (count) => (count > 0 ? BADGE_ROOM : 0));
  for (const step of steps) {
    for (const t of step.rows.keys()) own[t] = Math.max(own[t], lead[t] + rowWidth(step, t));
  }
  const width = new Float64Array(tree.count);
  const start = new Float64Array(tree.count);
  const x = new Float64Array(tree.count);
  const y = new Float64Array(tree.count);
  for (let i = tree.count - 1; i >= 0; i--) {
    const t = tree.order[i];
    width[t] = isLeaf(tree, t)
      ? LEAF_WIDTH
      : Math.max(own[t], width[tree.left[t]] + SIBLING_GAP + width[tree.right[t]]);
  }
  start[tree.root] = MARGIN;
  for (const t of tree.order) {
    y[t] = MARGIN + 10 + tree.depth[t] * ROW;
    if (isLeaf(tree, t)) continue;
    const [left, right] = [tree.left[t], tree.right[t]];
    start[left] = start[t] + (width[t] - width[left] - SIBLING_GAP - width[right]) / 2;
    start[right] = start[left] + width[left] + SIBLING_GAP;
  }
  for (let i = tree.count - 1; i >= 0; i--) {
    const t = tree.order[i];
    x[t] = isLeaf(tree, t) ? start[t] + width[t] / 2 : (x[tree.left[t]] + x[tree.right[t]]) / 2;
  }
  return {
    lead,
    start,
    width,
    x,
    y,
    totalWidth: width[tree.root] + 2 * MARGIN,
    totalHeight: 2 * MARGIN + 30 + tree.height * ROW,
  };
}

function make(name, attributes, text) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
  if (text !== undefined) node.textContent = text;
  return node;
}

const curve = (x1, y1, x2, y2) => {
  const bend = (y2 - y1) / 2;
  return `M${x1},${y1} C${x1},${y1 + bend} ${x2},${y2 - bend} ${x2},${y2}`;
};

// Draws `scene` into `svg`:
//   pieces: [{step, values, fresh}], the diagrams on show, with the value of
//     each of their nodes, and whether each is the step just taken;
//   clauses: per vtree node, {count, placed} for the clauses that wait there;
//   evidence: per variable, 1 or -1 for pinned true or false, 0 for free;
//   format: a node value as text.
// Leaf labels carry `data-variable` and clause badges `data-node`, for the
// page's pointer handlers. Returns the zoom the picture is drawn at.
export function render(svg, tree, place, scene) {
  const { x, y } = place;
  svg.replaceChildren();
  const room = svg.parentElement.clientWidth - 2;
  const zoom = Math.min(MOST_ZOOM, Math.max(LEAST_ZOOM, room / place.totalWidth));
  svg.setAttribute("viewBox", `0 0 ${place.totalWidth} ${place.totalHeight}`);
  svg.setAttribute("width", place.totalWidth * zoom);
  svg.setAttribute("height", place.totalHeight * zoom);
  const layers = ["skeleton", "badges", "edges", "nodes", "leaves"].map((name) => make("g", { class: name }));
  const [skeleton, badges, edges, nodes, leaves] = layers;
  svg.append(...layers);

  for (const t of tree.order) {
    if (isLeaf(tree, t)) {
      const v = tree.variable[t];
      const pin = scene.evidence[v];
      const label = make("g", { class: pin > 0 ? "leaf true" : pin < 0 ? "leaf false" : "leaf", "data-variable": v });
      const text = `x${v}`;
      const boxWidth = Math.max(26, text.length * 7 + 10);
      label.append(
        make("rect", { x: x[t] - boxWidth / 2, y: y[t] - 10, width: boxWidth, height: 20, rx: 10 }),
        make("text", { x: x[t], y: y[t] + 4 }, text),
      );
      leaves.append(label);
      continue;
    }
    for (const child of [tree.left[t], tree.right[t]]) {
      skeleton.append(make("line", { x1: x[t], y1: y[t], x2: x[child], y2: y[child] }));
    }
    skeleton.append(make("circle", { cx: x[t], cy: y[t], r: 5 }));
  }

  // Each drawn node's centre, by level and node, and where the drawing at
  // each level starts, which the count of clauses waiting there goes before.
  const centres = new Map();
  const leftEdge = new Map();
  for (const { step } of scene.pieces) {
    for (const [t, row] of step.rows) {
      const width = rowWidth(step, t);
      const [first, last] = [place.start[t] + place.lead[t], place.start[t] + place.width[t] - width];
      let left = Math.min(Math.max(x[t] - width / 2, first), last);
      leftEdge.set(t, left);
      for (const i of row) {
        const w = nodeWidth(step.diagram.levels[t][i].length);
        centres.set(`${t} ${i}`, left + w / 2);
        left += w + NODE_GAP;
      }
    }
  }

  for (const { step, values, fresh } of scene.pieces) {
    const group = (layer) => {
      const g = make("g", fresh ? { class: "fresh" } : {});
      layer.append(g);
      return g;
    };
    const [edgeGroup, nodeGroup] = [group(edges), group(nodes)];
    const { top } = step;
    if (step.rows.size === 0) {
      // A diagram with nothing to draw: false, or true of everything.
      const symbol = step.node === null ? "⊥" : "⊤";
      const mark = make("g", { class: step.node === null ? "constant bottom" : "constant top" });
      mark.append(make("circle", { cx: x[top], cy: y[top], r: 11 }), make("text", { x: x[top], y: y[top] + 5 }, symbol));
      leftEdge.set(top, x[top] - 11);
      nodeGroup.append(mark);
      continue;
    }
    for (const [t, row] of step.rows) {
      const [left, right] = [tree.left[t], tree.right[t]];
      for (const i of row) {
        const pairs = step.diagram.levels[t][i];
        const cx = centres.get(`${t} ${i}`);
        const cy = y[t];
        const value = values[t][i];
        const node = make("g", { class: value === 0 ? "node dead" : t === top && fresh ? "node top" : "node" });
        let pairX = cx - nodeWidth(pairs.length) / 2;
        for (let k = 0; k < pairs.length; k += 2) {
          const dead = values[left][pairs[k]] * values[right][pairs[k + 1]] === 0;
          const pairCentre = pairX + PAIR_WIDTH / 2;
          const pairTop = cy + PAIR_DROP;
          edgeGroup.append(
            make("line", { x1: cx, y1: cy + NODE_RADIUS, x2: pairCentre, y2: pairTop, class: dead ? "dead" : "" }),
          );
          const pair = make("g", { class: dead ? "pair dead" : "pair" });
          pair.append(make("rect", { x: pairX, y: pairTop, width: PAIR_WIDTH, height: CELL_HEIGHT, rx: 3 }));
          pair.append(make("line", { x1: pairCentre, y1: pairTop, x2: pairCentre, y2: pairTop + CELL_HEIGHT }));
          [
            [left, pairs[k], pairX + CELL_WIDTH / 2],
            [right, pairs[k + 1], pairX + PAIR_WIDTH - CELL_WIDTH / 2],
          ].forEach(([child, j, cellX]) => {
            const cellY = pairTop + CELL_HEIGHT / 2;
            const word = sideWord(tree, step, child, j);
            if (word !== null) {
              pair.append(make("text", { x: cellX, y: cellY + 4 }, word));
              return;
            }
            pair.append(make("circle", { cx: cellX, cy: cellY, r: 2.5 }));
            const target = centres.get(`${child} ${j}`);
            edgeGroup.append(
              make("path", { d: curve(cellX, cellY, target, y[child] - NODE_RADIUS), class: dead ? "dead" : "" }),
            );
          });
          node.append(pair);
          pairX += PAIR_WIDTH + PAIR_GAP;
        }
        node.append(make("circle", { cx, cy, r: NODE_RADIUS }));
        node.append(make("text", { x: cx + NODE_RADIUS + 4, y: cy - 4, class: "value" }, scene.format(value)));
        nodeGroup.append(node);
      }
    }
  }

  for (let t = 0; t < tree.count; t++) {
    const clauses = scene.clauses[t];
    if (clauses.count === 0) continue;
    const text = String(clauses.count);
    const badgeWidth = 10 + 7 * text.length;
    const right = (leftEdge.get(t) ?? x[t] - 5) - 6;
    const badge = make("g", { class: clauses.placed ? "badge placed" : "badge", "data-node": t });
    badge.append(
      make("rect", { x: right - badgeWidth, y: y[t] - 8, width: badgeWidth, height: 16, rx: 8 }),
      make("text", { x: right - badgeWidth / 2, y: y[t] + 4 }, text),
    );
    badges.append(badge);
  }
  return zoom;
}

// What a pair's side reads as when it is not drawn as an edge: a literal, or
// ⊤ for a side every assignment satisfies. null for a drawn node.
function sideWord(tree, step, t, i) {
  if (isLeaf(tree, t)) {
    const v = tree.variable[t];
    return i === TRUE ? "⊤" : i === POSITIVE ? `x${v}` : i === NEGATIVE ? `¬x${v}` : null;
  }
  return isFree(tree, step, t, i) ? "⊤" : null;
}
