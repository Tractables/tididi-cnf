# Choosing a vtree

A TDD tests its variables in the shape of a vtree, a binary tree over them,
and the same formula can give a small circuit on one vtree and a huge one on
another. This example compiles one formula on four vtrees and compares the
circuits. [`count`](crate::count) chooses a vtree by itself; the steps below
are for a caller who wants to choose.

Run the complete program with `cargo run --example vtree_choice`.

## A formula whose numbering misleads

Two 12-bit registers `x` and `y` hold the same value, and it is not zero.
Variable `i` is bit `i` of `x` and variable `i + 12` the same bit of `y`.
Each bit gives two clauses, `¬xᵢ ∨ yᵢ` and `xᵢ ∨ ¬yᵢ`, and one more clause,
`x₁ ∨ … ∨ x₁₂`, says some bit of `x` is set:

```rust,ignore,{class=tested-example}
use tididi_cnf::compile::{compile_cnf, CompileOptions};
use tididi_cnf::parse_dimacs;
use tididi_cnf::vtree::{build_vtree, VtreeOptions};
```

```rust,ignore,{class=tested-example}
let bits = 12;
let mut dimacs = format!("p cnf {} {}\n", 2 * bits, 2 * bits + 1);
for i in 1..=bits {
    let j = i + bits;
    dimacs += &format!("-{i} {j} 0\n{i} -{j} 0\n");
}
let nonzero: Vec<String> = (1..=bits).map(|i| i.to_string()).collect();
dimacs += &format!("{} 0\n", nonzero.join(" "));
let (formula, _header) = parse_dimacs(dimacs.as_bytes())?;
```

Each bit of `y` depends only on its own bit of `x`, but the numbering puts
every bit of `x` before every bit of `y`.

## Compile on each vtree

[`build_vtree`](crate::vtree::build_vtree) builds a vtree from a spec, the
`--vtree` argument of the binary, and
[`compile_cnf`](crate::compile::compile_cnf) compiles the formula on it
inside the vtree's context:

```rust,ignore,{class=tested-example}
for spec in ["linear", "balanced", "minfill-primal", "portfolio"] {
    let vtree = build_vtree(&formula, spec, &VtreeOptions::default())?;
    let diagram = vtree
        .context()
        .run(|engine| compile_cnf(engine, &formula, &vtree, CompileOptions::default()))?
        .diagram;
    println!("{spec:>14}: {:>5} nodes, {} models", diagram.node_count(), diagram.model_count()?);
}
```

Output:

```text
        linear: 12282 nodes, 4095 models
      balanced:  8543 nodes, 4095 models
minfill-primal:   119 nodes, 4095 models
     portfolio:    46 nodes, 4095 models
```

Every circuit counts the same 4095 models, one per nonzero value. `linear`
follows the numbering, as an OBDD's variable order would: once it has read
all of `x` it must remember the whole value to check `y` against it, one
path for each of its 4095 values. `balanced` splits the numbering in half at
its root, `x` from `y`, and has the same value to carry across that split.
`minfill-primal` reads the clauses instead: an elimination order on the graph
of variables that share a clause puts each bit of `x` beside its bit of `y`,
and the circuit needs about ten nodes per bit. `portfolio`, the default, builds
several such vtrees, scores them against the formula and keeps the best.

Without the nonzero clause the formula falls apart into twelve independent
pairs, and the compiler compiles each pair on its own: every vtree then gives
the same small circuit.

The specs and the constructions behind them are documented with
[vitri](https://github.com/Tractables/vitri/blob/main/docs/vtrees.md).

Continue with the
[configuration example](crate::guide::examples::catalog), which
counts with the vtree chosen for you, or see the
[complete program](https://github.com/Tractables/tididi-cnf/blob/v0.1.2/examples/vtree_choice.rs).
