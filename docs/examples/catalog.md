# Count product configurations

A laptop shop sells one of three processors, one of three memory sizes, and
optionally a graphics card and a larger battery. Some combinations are not
for sale: the graphics card needs at least 16 GB of memory, 32 GB comes only
with the third processor, and the larger battery does not fit beside the
graphics card. How many laptops can a customer order, and how many remain as
they choose?

Run the complete program with `cargo run --example catalog`.

## Write the rules as clauses

A configurator usually hands over its rules in DIMACS CNF, the format the
`tididi` binary reads. Each variable is one option: 1 to 3 choose the
processor, 4 to 6 choose 8, 16 or 32 GB, 7 adds the graphics card and 8 the
larger battery. Each line ending in `0` is a clause, a disjunction of
literals: `1 2 3` says some processor is chosen, `-1 -2` that the first two are
not both chosen, `-7 5 6` that a graphics card needs 16 or 32 GB, `-6 3` that
32 GB needs the third processor, and `-8 -7` that the battery and the card
exclude each other.

```rust,ignore,{class=tested-example}
let rules = "\
p cnf 8 11
1 2 3 0
-1 -2 0
-1 -3 0
-2 -3 0
4 5 6 0
-4 -5 0
-4 -6 0
-5 -6 0
-7 5 6 0
-6 3 0
-8 -7 0
";
```

## Count the configurations

[`count`](crate::count) answers one task on one instance. The instance can be
a file path or, as here, a formula parsed with
[`parse_dimacs`](crate::parse_dimacs). The task is
[`Mode::Mc`](crate::Mode::Mc), the model count: the number of assignments to
all eight variables that satisfy every clause.

```rust,ignore,{class=tested-example}
let limits = Limits::default();
let answer = tididi_bonsai::count(parse_dimacs(rules.as_bytes())?, Some(Mode::Mc), &limits)?;
println!("Valid configurations: {}", answer.count());
```

Output:

```text
Valid configurations: 18
```

Check it by hand: 8 GB goes with any processor and the card is excluded, so
3 × 2 battery choices; 16 GB goes with any processor, with the card or the
battery or neither, so 3 × 3; 32 GB only with the third processor, again 3
ways. That makes 6 + 9 + 3 = 18.

## Count only some of the options

The shop's stock planner asks a different question: how many processor and
memory pairs can be sold at all, whatever the extras? That is a **projected**
model count. A `c p show` line names the variables to count over, and an
assignment to them counts once if some choice of the other variables completes
it to a valid configuration.

```rust,ignore,{class=tested-example}
let hardware = format!("{rules}c p show 1 2 3 4 5 6 0\n");
let answer = tididi_bonsai::count(parse_dimacs(hardware.as_bytes())?, Some(Mode::Pmc), &limits)?;
println!("Processor and memory pairs: {}", answer.count());
```

Output:

```text
Processor and memory pairs: 7
```

Three processors with 8 GB, three with 16 GB, and one with 32 GB.

## Keep the circuit for a customer's choices

A counting task builds what it needs and discards it. A compile,
[`Mode::Compile`](crate::Mode::Compile), keeps the result: a Tree Decision
Diagram over the formula's own variables, which the
[`tididi`](crate::tididi) library queries. A counter on it answers each new
choice without compiling again; `observe` fixes literals, replacing an
earlier choice of the same variable.

```rust,ignore,{class=tested-example}
let answer = tididi_bonsai::count(parse_dimacs(rules.as_bytes())?, Some(Mode::Compile), &limits)?;
let rules_circuit = answer.diagram().expect("a compile keeps its circuit");
let mut counter = rules_circuit.counter()?;
counter.observe([Literal::try_from(7)?])?;
println!("With a graphics card: {}", counter.model_count()?);
counter.observe([Literal::try_from(1)?])?;
println!("And the first processor: {}", counter.model_count()?);
counter.observe([Literal::try_from(-7)?])?;
println!("First processor, no graphics card: {}", counter.model_count()?);
```

Output:

```text
With a graphics card: 4
And the first processor: 1
First processor, no graphics card: 4
```

With the card, the battery is out and memory is 16 GB (any processor) or
32 GB (the third): 4 laptops. The first processor then leaves only 16 GB.
Dropping the card again gives 8 or 16 GB, each with or without the battery.

## Add a rule to the circuit

The shop now offers only laptops with a graphics card or a larger battery.
Build that clause as a diagram on the compiled circuit's own vtree, then
conjoin the diagrams with [`tididi::and`](crate::tididi::and). The compiler
may return a different vtree from the one it started with, so new operands
use the result's tree. Cloning keeps the original circuit available for
other queries; the counter's observations belong to that counter and do not
change the circuit.

```rust,ignore,{class=tested-example}
let extra = Tdd::clause(rules_circuit.vtree(), [7, 8])?;
let upgraded = tididi::and(rules_circuit.clone(), extra)?;
println!("With a graphics card or larger battery: {}", upgraded.model_count()?);
```

Output:

```text
With a graphics card or larger battery: 11
```

Four configurations have a graphics card, and seven have the larger battery.
The rules forbid having both, so the new offer has 4 + 7 = 11 configurations.

To keep the circuit between runs, save it as the binary's `--save-tdd` does;
the library's
[persistence example](https://tractables.github.io/tididi/tididi/guide/examples/persistence/index.html)
saves and reloads one. The
[complete program](https://github.com/Tractables/tididi-bonsai/blob/v0.1.0/examples/catalog.rs)
runs all these queries.
