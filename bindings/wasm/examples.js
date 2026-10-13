// The row of examples: a key, which is how the address bar names one, the
// name on its button, and its file, one of the formulas under tests/data/,
// which site.sh puts beside the page. The smoke test counts each of them and
// compares the count with the native binary's.

export const EXAMPLES = [
  ["simple", "three clauses", "simple.cnf"],
  ["queens", "six queens", "queens6.cnf"],
  ["pigeons", "eight pigeons, seven holes", "pigeon8_7.cnf"],
  ["weighted", "weighted", "wmc_header_tiny_sat.cnf"],
  ["projected", "projected", "pmc_tiny.cnf"],
  ["projected-weighted", "projected and weighted", "pwmc_arjun_defined_weight.cnf"],
];

// This file's version stamp (see README.md), passed on to the files it
// fetches.
const stamp = new URL(import.meta.url).search;

export function fetchExample(file) {
  return fetch(file + stamp).then((response) => {
    if (!response.ok) throw new Error(`${response.status} for ${file}`);
    return response.text();
  });
}
