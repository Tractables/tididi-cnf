"""Formulas the tests share, and the instances the bindings compare with the
binary's counts."""

import pathlib

PACKAGE = pathlib.Path(__file__).resolve().parents[1]
REPOSITORY = PACKAGE.parents[1]
EXAMPLES = PACKAGE / "examples"

# (x1 or x2) and (not x1 or x3) and (not x2 or not x3): two models.
TWO_MODELS = "p cnf 3 3\n1 2 0\n-1 3 0\n-2 -3 0\n"
TWO_MODELS_MC = "c t mc\n" + TWO_MODELS
REFUTED = "p cnf 1 2\n1 0\n-1 0\n"
# x1 or x2, with x1 weighing 3/10 true and 7/10 false and x2 1/2 either way:
# 3/20 + 3/20 + 7/20 = 13/20.
WEIGHTED = (
    "c t wmc\np cnf 2 1\nc p weight 1 3/10 0\nc p weight -1 7/10 0\n"
    "c p weight 2 1/2 0\nc p weight -2 1/2 0\n1 2 0\n"
)
# (not x1 or x3) and (x1 or x2), counted over x1 and x2: the assignments TT,
# TF and FT extend to a model, and weigh 1/12 + 2/12 + 2/12 = 5/12.
PROJECTED_WEIGHTED = (
    "c t pwmc\np cnf 3 2\nc p show 1 2 0\n"
    "c p weight 1 1/3 0\nc p weight -1 2/3 0\nc p weight 2 1/4 0\nc p weight -2 1/2 0\n"
    "-1 3 0\n1 2 0\n"
)
MALFORMED = "p cnf 2 1\n1 x 0\n"


def parity_instances():
    """The (path, mode) pairs bindings/parity.txt lists, which every binding
    counts and compares with the binary's answer."""
    pairs = []
    for line in (REPOSITORY / "bindings" / "parity.txt").read_text().splitlines():
        if line.strip() and not line.startswith("#"):
            path, mode = line.split()
            pairs.append((REPOSITORY / path, mode))
    return pairs


def clauses_of(text):
    """DIMACS text as the keywords `count` takes for clauses: `num_vars`,
    `show` and `weights`, with the clauses under `formula`. Reads the lines
    the instances bindings/parity.txt lists use; the weights stay the strings
    the file writes."""
    formula = {"formula": [], "num_vars": None, "show": None, "weights": {}}
    clause = []
    for line in text.splitlines():
        words = line.split()
        if words[:3] == ["c", "p", "show"]:
            formula["show"] = (formula["show"] or []) + [int(word) for word in words[3:-1]]
        elif words[:3] == ["c", "p", "weight"]:
            formula["weights"][int(words[3])] = words[4]
        elif words[:2] == ["p", "cnf"]:
            formula["num_vars"] = int(words[2])
        elif words and words[0] not in ("c", "%"):
            for literal in map(int, words):
                if literal:
                    clause.append(literal)
                else:
                    formula["formula"].append(clause)
                    clause = []
    assert not clause, "the last clause is not closed"
    return formula
