# Output

What a run prints, what it writes, and how it exits. `tididi` in the commands
is the built binary, `target/release/tididi`.

- **stdout** carries the answer, and nothing else unless `--stats -` asks for
  the record there too.
- **stderr** carries progress and errors, for people to read.
- **the exit status** says whether there is an answer.
- **`--save-tdd`** and **`--stats`** write files; [`stats.md`](stats.md) is
  the record's reference.

## The answer line

By default the answer is one line on stdout:

| mode | line | the count |
|---|---|---|
| `mc`, `compile` | `model count: 22` | a decimal integer |
| `pmc` | `projected model count: 3` | a decimal integer |
| `pwmc` | `projected weighted count: 22/25` | a fraction in lowest terms, `0/1` for zero |
| `wmc` | `weighted count: 3.2999999999999985e0` | `<mantissa>e<exponent>`, the mantissa in [1, 10) and signed when negative; `0` for zero |

With `--save-tdd` a compile prints a second line, naming the files:

```text
model count: 22
tdd written to switches.tdd, vtree to switches.tdd.vtree: 9 nodes, 16 pairs
```

A run that ends without an answer prints nothing on stdout.

## The competition's output format

`--output-format mcc` prints the answer as the model counting competition's
output format has it
([version 1.2](https://mccompetition.org/assets/files/mccomp_format_25.pdf),
§3): a status line, the task, the base-10 logarithm of the count, and the
count.

```sh
tididi --output-format mcc tests/data/mcc_mc_sat.cnf
```

Output:

```text
s SATISFIABLE
c s type mc
c s log10-estimate 1.3424226808222062
c s exact arb int 22
```

```sh
tididi --output-format mcc tests/data/wmc_header_tiny_sat.cnf
```

Output:

```text
s SATISFIABLE
c s type wmc
c s log10-estimate 0.5185139398778873
c s exact double prec-sci 3.2999999999999985e0
```

```sh
tididi --output-format mcc tests/data/mcc_pmc_sat.cnf
```

Output:

```text
s SATISFIABLE
c s type pmc
c s log10-estimate 0.47712125471966244
c s exact arb int 3
```

```sh
tididi --output-format mcc tests/data/mcc_pwmc_sat.cnf
```

Output:

```text
s SATISFIABLE
c s type pwmc
c s log10-estimate -0.055517327849831544
c s exact arb frac 22/25
```

A compile answers as `c s type mc`, since its count is a model count, and
prints no `tdd written to` line.

- **The status line** says whether the formula has a model. It is
  `s UNSATISFIABLE` exactly when there is none, and it is not read off the
  count: weights of zero, or weights that cancel, make a satisfiable
  formula's weighted count 0.
- **The estimate** is `log10-estimate -inf` for a count of 0, and a negative
  count prints `neglog10-estimate` with the logarithm of its absolute value.
  `tests/data/mcc_wmc_negative.cnf` counts to −3/8:

  ```text
  s SATISFIABLE
  c s type wmc
  c s neglog10-estimate -0.4259687322722811
  c s exact double prec-sci -3.75e-1
  ```

- **The last line** names how the count was computed: `exact arb int` for an
  integer count, `exact arb frac` for the exact projected weighted count, and
  `exact double prec-sci` for the weighted model count, which is computed in
  double precision.
- **A run that gives up** prints `s UNKNOWN` and nothing more
  ([`limits.md`](limits.md#giving-up)).

## The saved circuit

`--save-tdd <file>` writes the compiled diagram to `<file>` in the tididi
library's text format, and its vtree to `<file>.vtree` in the vtree text
format. Only the compile mode builds a circuit whose count is the one
reported, so the counting modes refuse the flag. The library reads a `.tdd`
file over the vtree it was compiled on, `load_tdd(path, &vtree)`, and checks
the file against that vtree, so reading the circuit back takes both files:
parse the vtree, then read the circuit over it.

```rust
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let vtree = tididi::Vtree::from_text(&std::fs::read_to_string("switches.tdd.vtree")?)?;
    let circuit = tididi::io::load_tdd("switches.tdd", &Arc::new(vtree))?;
    println!("{}", circuit.model_count()?);
    Ok(())
}
```

This prints `22` for the circuit that
[getting started](getting-started.md#compile-for-later-queries) saves; the
test `save_tdd_reloads_through_the_library` in `tests/cli.rs` makes the same
calls. The comment lines at the top of a `.tdd` file describe its records,
and the library's `io` module documents the format. The circuit carries no
weights: weigh it again in the library. The same module draws a circuit and
its vtree for Graphviz (`tdd_to_dot`, `vtree_to_dot`); the binary has no flag
for it.

## Exit status

| status | when | `exit_reason` in the record |
|---|---|---|
| 0 | an answer was printed; also `--help` and `--version` | `solved` |
| 1 | the run gave up without an answer | `incomplete` |
| 1 | the input could not be opened or parsed | `input` |
| 1 | the `--vtree` spec cannot serve this run | `vtree-spec` |
| 1 | a file the run was asked to write could not be written | `artifact` |
| 2 | the command line is wrong, or names a flag the mode would ignore | `usage` |
| 2 | an environment variable is invalid ([`env.md`](env.md)) | `environment` |
| 2 | preprocessing or a compile stage failed | `stage` |

Every failure ends with one message on stderr, after `error: `, that says
what went wrong. A run refused before its mode starts, such as a command
line the parser cannot read, writes no record. A `--stats` file that cannot
be written fails a run that has already printed its answer, with status 1:

```sh
tididi --stats /nonexistent/dir/x.json tests/data/mcc_mc_sat.cnf
```

The answer `model count: 22` is on stdout, and the last line on stderr is
`error: could not write /nonexistent/dir/x.json: No such file or directory (os error 2)`.

A reader that stops early, such as `head` or a pager that quits, closes
stdout under the run. That is not a failure: the run goes on without stdout,
writes its other files, and ends with the status it would have had.

## Stderr

Stderr is progress, not a contract. Each line is `[<mode>] <stage>: …` for a
stage of one mode, or `[<subsystem>] …` for a part the modes share, such as
`[input]`, `[vtree]` or `[compile]`; an error starts `error:`. In a terminal,
stderr and the answer interleave:

```sh
tididi tests/data/mcc_mc_sat.cnf
```

```text
[input] reading tests/data/mcc_mc_sat.cnf
[input] 6 variables, 4 clauses
[mc] preprocessing: start
[mc] arjun: reduced in 4ms — 6v/4c → 6v/4c, lift=2^0
[mc] compile: start on force
[mc] answered by compile in 0ms
model count: 22
```

The wording, the stages named and the timings change between runs and
releases. To read a run in a program, parse the answer from stdout, and
everything else from the `--stats` record ([`stats.md`](stats.md)).
