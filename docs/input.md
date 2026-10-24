# Input format

`tididi` reads one DIMACS CNF file, named on the command line, with the header
lines of the model counting competition's format
([version 1.2](https://mccompetition.org/assets/files/mccomp_format_25.pdf),
§2) for the task, the show set and the weights. It also reads a directory
that vitri wrote ([below](#a-vitri-bundle)), and no other input: not stdin,
and not compressed files, so decompress a `.cnf.xz` with `xz -dk` first.
`tididi` in the commands is the built binary, `target/release/tididi`.

vitri reads the file for the compiler, so the two accept the same inputs; the
grammar below is vitri's `CnfFormula::from_dimacs`.

## DIMACS

```text
c t mc
p cnf 6 4
-1 -2 0
2 3 -4 0
4 5 0
4 6 0
```

- **`p cnf <variables> <clauses>`** is required in a file that has clauses,
  and may come anywhere in it. Variables are numbered from 1 to
  `<variables>`, and a file that names a higher one, in a clause, a show line
  or a weight line, is refused. The clause count is not checked: extra
  clauses are read.
- **A clause** is a list of nonzero integers ended by `0`; `-3` is the
  negation of variable 3. A clause may span lines, and the last clause may
  omit its `0`. A literal that repeats is read once, and a clause that
  contains a variable and its negation is dropped, since every assignment
  satisfies it.
- **A `0` with no literal before it** is the empty clause. The formula is
  unsatisfiable, and every mode answers 0.
- **Lines starting with `c`** are comments, except the header lines below.
  Blank lines are skipped, and a line starting with `%` ends the file.
- **A line starting with `w`, or whose first token has a decimal point,** is
  skipped as another format's weight line.
- **Every declared variable counts,** whether or not a clause mentions it
  ([`tasks.md`](tasks.md#the-five-modes)).

A file that breaks these rules stops the run with exit status 1 and a message
that names the line:

```sh
printf 'p cnf 2 1\n1 3 0\n' > over.cnf
tididi over.cnf
```

Stderr:

```text
[input] reading over.cnf
error: could not parse over.cnf: line 2: clause literal 3 exceeds the declared variable count 2
```

## Header lines

The competition's format adds three kinds of `c` line. They may appear
anywhere in the file.

| line | meaning |
|---|---|
| `c t <mc\|wmc\|pmc\|pwmc>` | the task. Any other type is refused. |
| `c p show <v1> <v2> … 0` | variables to project onto. Several lines add up to one show set, and `c p show 0` declares an empty one. |
| `c p weight <literal> <weight> 0` | the weight of one literal |

[`tasks.md`](tasks.md#how-a-run-picks-its-mode) says how these lines and
`--mode` pick the task. A show line makes the count projected, and a weight
line on its own changes nothing: weights count only when the mode is `wmc`
or `pwmc`.

## Weights

A weight is a decimal (`0.4`), a number in scientific notation (`1.5e-1`) or
a fraction (`3/10`). Weights are read as exact fractions, and may be zero or
negative.

The two literals of a variable are weighed as the competition's format
defines:

- **both have a weight line:** each weighs what its line gives it;
- **neither has one:** both weigh 1;
- **one has a line, with a weight `w` strictly between 0 and 1:** the other
  weighs `1 - w`;
- **one has a line, with any other weight:** the format gives the other
  literal no weight, and the file is refused.

In `half.cnf`, only `1` has a weight line, so `-1` weighs 0.7:

```sh
printf 'c t wmc\np cnf 2 1\nc p weight 1 0.3 0\n1 2 0\n' > half.cnf
tididi half.cnf
```

Output:

```text
weighted count: 1.2999999999999996e0
```

The three models weigh 0.3 (both true), 0.3 (x1 true, x2 false) and 0.7
(x1 false), which makes 1.3; x2 has no weight line and weighs 1 either way.
Adding `c p weight -1 0.7 0` gives the same answer.

A weight of 0 or less, or of 1 or more, on one literal alone stops the run
with exit status 1, in every mode, and the message names the literal:

```sh
printf 'c t wmc\np cnf 2 1\nc p weight 1 3/2 0\n1 2 0\n' > lone.cnf
tididi lone.cnf
```

Stderr:

```text
[input] reading lone.cnf
error: could not parse lone.cnf: weight literal 1 has weight 3/2 and literal -1 has none; the Model Counting Competition format defines the missing weight as 1 - w only for 0 < w < 1, so give both literals
```

To use such a weight, write a weight line for both literals of the variable.
In a projected weighted count, only the show variables' weights count
([`tasks.md`](tasks.md#the-five-modes)).

## A vitri bundle

A directory is read as the bundle `vitri --out-dir` writes
([vitri's bundle guide](https://github.com/Tractables/vitri/blob/main/docs/bundle.md)),
and the original formula is not needed:

```sh
vitri formula.cnf --out-dir bundle/ --mode mc
tididi bundle/
```

The run compiles `reduced.cnf` on `vtree.vtree`, one tree per component when
`components.json` lists several, and preprocesses nothing. The mode is the one
`preprocess.json` records, and the answer is the original formula's: the
record's lift is applied to the count, and a compile's diagram is over the
original variables.
