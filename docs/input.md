# Input format

`tididi` reads one DIMACS CNF file, named on the command line, with the header
lines of the model counting competition's format
([version 1.2](https://mccompetition.org/assets/files/mccomp_format_25.pdf),
§2) for the task, the show set and the weights. It reads no other input: not
stdin, and not compressed files, so decompress a `.cnf.xz` with `xz -dk`
first. `tididi` in the commands is the built binary,
`target/release/tididi`.

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
line on its own changes nothing: weights are read only when the mode is `wmc`
or `pwmc`.

## Weights

A weight is a decimal (`0.4`), a number in scientific notation (`1.5e-1`) or
a fraction (`3/10`). Weights are read as exact fractions, and may be zero or
negative.

**A literal with no weight line weighs 1, even when the other literal of its
variable has one.** The competition's format reads a lone
`c p weight 1 0.3 0` as also giving `-1` the weight 0.7; `tididi` gives `-1`
the weight 1, and prints no warning:

```sh
printf 'c t wmc\np cnf 2 1\nc p weight 1 0.3 0\n1 2 0\n' > half.cnf
tididi half.cnf
```

Output:

```text
weighted count: 1.5999999999999999e0
```

The three models weigh 0.3 (both true), 0.3 (x1 true, x2 false) and 1
(x1 false), which makes 1.6. With `c p weight -1 0.7 0` added, `tididi`
prints `weighted count: 1.2999999999999996e0`, the 1.3 of the competition's
reading. Write a weight line for both literals of every weighted variable, and
the two readings agree.

A variable with no weight line at all weighs 1 either way, as the
competition's format also has it. In a projected weighted count, only the
show variables' weights count ([`tasks.md`](tasks.md#the-five-modes)).
