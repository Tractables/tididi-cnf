// Tree Decision Diagrams in the library's `.tdd` text format, read against
// their vtree, and the values a diagram gives its nodes. No DOM; the smoke
// test imports this file too.
//
// A diagram has one level per vtree node. A leaf level offers three nodes: 0
// is true, 1 the variable's positive literal and 2 its negative literal. A
// node of an internal level is a list of pairs, each joining a node of the
// left child level to a node of the right one: the pair is the conjunction
// of the two, and the node the disjunction of its pairs. The vtree is what
// vitri's `parseVtree` (vtree.js) returns.

import { isLeaf, isUnder } from "./vtree.js";

// The nodes of a leaf level.
export const TRUE = 0;
export const POSITIVE = 1;
export const NEGATIVE = 2;

const isCount = (token) => /^\d+$/.test(token);

// The diagram `text` holds, on `tree`: `levels[t]` lists the nodes of
// internal level t, each an Int32Array of its pairs [l0, r0, l1, r1, ...],
// and `output` is the output node of the root level, or null for the false
// diagram. The checks are that the records describe `tree`, that every pair
// names nodes that exist, and that the output does. Returns {error} for text
// that is not such a diagram.
export function parseTdd(text, tree) {
  const levels = Array.from({ length: tree.count }, () => []);
  const size = (t) => (isLeaf(tree, t) ? 3 : levels[t].length);
  let output;
  let problem = false;
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line === "" || line.startsWith("c")) continue;
    const fields = line.split(/\s+/);
    const where = `line ${i + 1}`;
    if (!problem) {
      const numbers = fields.slice(2);
      const counts = numbers.every((field, k) => isCount(field) || (k === 4 && field === "ZERO"));
      if (fields[0] !== "p" || fields[1] !== "tdd" || numbers.length !== 5 || !counts) {
        return { error: `${where} is not the p tdd line` };
      }
      const [version, leaves, nodes, outVtree] = numbers.slice(0, 4).map(Number);
      if (version !== 1) return { error: `format version ${version} is not one this page reads` };
      if (leaves !== tree.leaves[tree.root] || nodes !== tree.count || outVtree !== tree.root) {
        return { error: "the diagram is on another vtree" };
      }
      output = fields[6] === "ZERO" ? null : Number(fields[6]);
      problem = true;
      continue;
    }
    const [kind, ...numbers] = fields;
    if (!numbers.every(isCount)) return { error: `${where} holds a field that is not a number` };
    const [a, b, c, ...pairs] = numbers.map(Number);
    if (kind === "L") {
      if (numbers.length !== 2 || a >= tree.count || !isLeaf(tree, a) || tree.variable[a] !== b) {
        return { error: `${where} names a leaf the vtree does not have` };
      }
    } else if (kind === "I") {
      if (numbers.length < 3 || a >= tree.count || isLeaf(tree, a) || tree.left[a] !== b || tree.right[a] !== c) {
        return { error: `${where} names an internal node the vtree does not have` };
      }
      if (pairs.length === 0 || pairs.length % 2 !== 0) return { error: `${where} does not hold whole pairs` };
      if (pairs.some((node, k) => node >= size(k % 2 === 0 ? b : c))) {
        return { error: `${where} pairs a node its child level does not have` };
      }
      levels[a].push(Int32Array.from(pairs));
    } else {
      return { error: `${where} is not a p, L or I record` };
    }
  }
  if (!problem) return { error: "there is no p tdd line" };
  if (output !== null && output >= size(tree.root)) return { error: "the output node does not exist" };
  return { levels, output };
}

// The value of every node, `values[t][node]`: `leaf(variable)` gives a leaf
// level's three, and a pair is worth the product of its two sides and a node
// the sum of its pairs. With 2 for true and 1 for each literal, a node's value
// is the number of assignments to the variables under its level that satisfy
// it.
export function nodeValues(diagram, tree, leaf) {
  const values = new Array(tree.count);
  // The preorder walk, backwards, meets children before their parents.
  for (let i = tree.count - 1; i >= 0; i--) {
    const t = tree.order[i];
    if (isLeaf(tree, t)) {
      values[t] = leaf(tree.variable[t]);
      continue;
    }
    const left = values[tree.left[t]];
    const right = values[tree.right[t]];
    values[t] = diagram.levels[t].map((pairs) => {
      let sum = 0;
      for (let k = 0; k < pairs.length; k += 2) sum += left[pairs[k]] * right[pairs[k + 1]];
      return sum;
    });
  }
  return values;
}

// Each variable's [negative, positive] literal weights from the `weights` a
// drawing carries, exact rationals as text, or all 1 for null, indexed by
// variable from 1.
export function readWeights(weights, variables) {
  const rational = (text) => {
    const [numerator, denominator = "1"] = text.split("/");
    return Number(numerator) / Number(denominator);
  };
  const table = [[1, 1]];
  for (let v = 1; v <= variables; v++) table.push(weights === null ? [1, 1] : weights[v - 1].map(rational));
  return table;
}

// The leaf values under literal weights and evidence: `weights[v]` is
// variable v's [negative, positive] (both 1 to count models), and
// `evidence[v]` is 1 or -1 to pin v true or false, 0 to leave it free.
export function leafValues(weights, evidence) {
  return (v) => {
    const positive = evidence[v] < 0 ? 0 : weights[v][1];
    const negative = evidence[v] > 0 ? 0 : weights[v][0];
    return [positive + negative, positive, negative];
  };
}

// The value of the whole diagram, given its nodes' values.
export const diagramValue = (diagram, tree, values) => (diagram.output === null ? 0 : values[tree.root][diagram.output]);

// The diagram's value under `weights` and `evidence`, and for each variable
// the share of that value in which it is true, `shares[v]`: the variable's
// probability given the evidence when the weights are probabilities, and the
// fraction of the models consistent with the evidence that set it true when
// they are all 1. NaN when the value is 0.
export function marginals(diagram, tree, weights, evidence) {
  const valueUnder = (pins) => diagramValue(diagram, tree, nodeValues(diagram, tree, leafValues(weights, pins)));
  const value = valueUnder(evidence);
  const shares = new Float64Array(tree.maxVariable + 1);
  const pins = Int8Array.from(evidence);
  for (let v = 1; v <= tree.maxVariable; v++) {
    if (evidence[v] !== 0) {
      shares[v] = value === 0 ? NaN : evidence[v] > 0 ? 1 : 0;
      continue;
    }
    pins[v] = 1;
    shares[v] = valueUnder(pins) / value;
    pins[v] = 0;
  }
  return { value, shares };
}

// The node of level `top` that the output reaches, in a diagram of clauses
// whose variables all lie under `top`: every level on the way down holds one
// node of one pair, true on the side away from `top`. null for the false
// diagram.
export function nodeAt(diagram, tree, top) {
  if (diagram.output === null) return null;
  let t = tree.root;
  let node = diagram.output;
  while (t !== top) {
    const pairs = diagram.levels[t][node];
    if (pairs.length !== 2) throw new Error(`level ${t} branches above level ${top}`);
    const left = isUnder(tree, top, tree.left[t]);
    node = left ? pairs[0] : pairs[1];
    t = left ? tree.left[t] : tree.right[t];
  }
  return node;
}
