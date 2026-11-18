# Time and memory limits

A run can be limited in time and in memory. This page says what the two
limits do, what the defaults are, and what a run that reaches one prints.
`tididi` in the commands is the built binary, `target/release/tididi`.

## Time

`--time-limit-ms <ms>` limits the run, starting before the input is read.
The environment variable `TIDIDI_TIME_LIMIT_MS` sets the same limit when
the flag is absent; with both, the flag wins, but an unreadable value in the
variable is still refused ([`env.md`](env.md)).

- **The limit covers the whole run, in every mode.** Preprocessing, vtree
  construction and the compile each take a share of it, and so, in the
  counting modes, do the stages that take over when a compile gives up, up to
  the reading of the final count. A `compile` that reaches the limit gives
  up the run: its one retry follows a compile that ran out of memory, not one
  that ran out of time. A zero limit is already expired; even a shortcut
  answer gives up.
- **SAT equivalence probing**, part of vitri's simplification of the formula
  in `compile`, `mc` and `wmc`, gets a quarter of the limit, at most 30 s, and
  30 s when the run has no limit; probing that finishes in its budget reduces
  the formula the same way on every run, where a shorter budget would stop it
  at a different probe each time.
- **Without a time limit a run has none.** Each stage then keeps a fixed
  budget of its own, and a hard formula can run for as long as it takes. Set a
  limit for any run you do not watch.

The limit is cooperative: stages check it as they go, and the driver checks
again before accepting an answer. A step already in progress inside vitri or
the diagram library can finish after the deadline; its result is discarded
and the run gives up, and the library's `count` returns `Error::Incomplete`.
Where the limit must be hard, wrap the run in `timeout` with some slack; a
process killed from outside writes no `--stats` record.

## Memory

`--mem-limit-mb <MiB>` lowers the process's address-space limit
(`RLIMIT_AS`, the limit `ulimit -v` sets) when the run starts. **The default
is 30000 MiB**, and it applies without the flag.

- **The limit is on address space, not resident memory.** Every mapping
  counts, including memory reserved and not yet touched, so the process can
  reach the limit while using less physical memory than that.
- **On a machine with more memory, a large instance can give up at the
  default** where a higher limit would have let it finish. Raise it with
  `--mem-limit-mb`.
- **It only ever lowers the limit.** A tighter `ulimit -v` set before the run
  stays in force, and `--mem-limit-mb 0` leaves the limit as it was.
- **Set it to the memory the run really has.** A model count watches this
  limit: when an `mc` compile nears it, the run splits the formula on a
  variable and counts the two halves, rather than growing until an allocation
  fails. The other modes have no such split: they give up, or end on the
  failed allocation. A `compile` on the portfolio's vtree that gives up with
  time left compiles once more on a second vtree before it gives up
  ([`tasks.md`](tasks.md#vtrees)). A limit imposed from outside that the
  process cannot see, such as a container's or a batch scheduler's, does not
  start that split; the process is killed instead. Passing that same value as
  `--mem-limit-mb` lets the run see it. With `--mem-limit-mb 0` and no
  `ulimit -v`, nothing does.
- **Runs side by side each have their own limit.** Choose it so that the runs
  together fit in the machine's memory.

A run that an allocation failure ends says so on stderr with the allocator's
own words, `memory allocation of … bytes failed`, so that a script can tell
it from a timeout.

## Giving up

A run that reaches a limit without an answer gives up. It prints no answer
line, and under `--output-format mcc` it prints `s UNKNOWN`. Stderr says
which stage gave up, and its last line says what ended the run, such as
`error: no answer: the time limit was reached`. The exit status is 1, and the
record's `exit_reason` is `incomplete`.

For example, even a clause-free formula cannot answer under a zero limit:

```sh
printf 'c p show 1 0\np cnf 2 0\n' > free.cnf
tididi --mode pmc --time-limit-ms 0 --output-format mcc --stats - free.cnf
```

Stdout starts with `s UNKNOWN`, followed by the stats record with
`exit_reason` set to `incomplete`. The record keeps the input's mode and size;
it carries no count or satisfiability verdict. Stderr says that the time
limit was reached, and the process exits with status 1.

## Many instances

Each run uses one core, so a directory of instances is counted fastest with
several runs side by side, each with a time limit and a memory limit that
together fit the machine. This runs four at a time, ten minutes and 8000 MiB
each, and keeps each instance's answer, progress and record beside it:

```sh
xz -dk instances/*.cnf.xz
ls instances/*.cnf | xargs -P 4 -I{} sh -c \
  'tididi --output-format mcc --time-limit-ms 600000 --mem-limit-mb 8000 --stats {}.json {} > {}.out 2> {}.err'
```
