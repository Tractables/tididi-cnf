# Using diagrams from the compiler

The `tididi` crate owns diagrams and their operations, and the `vitri` crate
owns the formula: `vitri::cnf` defines `CnfFormula`, `Clause`, `Literal`, the
header metadata and the DIMACS reader and writer, and this crate takes those
types as they are. What this crate adds is `tididi_bonsai::compile` —
`compile_cnf` with its `CompileOptions`, `ComponentPlan` and `Output`, and
what it returns, `Compiled` or a `GiveUp` — and `tididi_bonsai::vtree`, the
vtree requests (`build_vtree` and `build_vtree_with_components`, with their
`VtreeOptions`, returning a tree or a `VtreeBuild`, and an `Error` when the
construction fails) and the two component descriptors, `ComponentVtree` and
`Component`. `tididi_bonsai::tididi` and `tididi_bonsai::vitri` re-export
the two libraries, so a caller uses the versions this crate compiles
against. The public `count` entry runs the same complete task as the binary
and returns an `Answer` under the caller's `Limits`; the individual counting
stages remain internal.

A diagram the compiler returns is an ordinary `Tdd`. The library's
[API overview](https://github.com/Tractables/tididi/blob/main/docs/api-guide.md)
covers composing, counting and querying it. A result may come back on a
rotated or grafted vtree, so build any new operand against the result's own
vtree rather than the one passed in; `compile_cnf`'s rustdoc says exactly when
the two differ.

## A bounded compile

The compiler checks out one `Engine` for the whole run and passes it through
compilation, queries and recovery, so one set of limits, memory hooks and
scratch covers every operation. To do the same, pass the engine that
`Context::run` or `Context::with_limits` supplies to `compile_cnf` and to every
query that should share its limits; a free function or diagram method starts
a separate checkout without them.

```rust
use std::sync::Arc;
use std::time::Instant;

use tididi_bonsai::compile::{CompileOptions, ComponentPlan, compile_cnf};
use tididi_bonsai::tididi::limits::LimitConfig;
use tididi_bonsai::tididi::{Context, Vtree};
use tididi_bonsai::vitri::cnf::CnfFormula;

/// The model count of `formula` compiled on `vtree`, in decimal, or `None`
/// when the compile or the count gives up by `deadline`.
fn compile_and_count(
    context: &Arc<Context>,
    formula: &CnfFormula,
    vtree: Vtree,
    deadline: Instant,
) -> Option<String> {
    let vtree = context.bind(vtree);
    context.with_limits(LimitConfig::none().with_deadline(Some(deadline)), |engine| {
        let opts = CompileOptions {
            plan: ComponentPlan::Whole,
            deadline: Some(deadline),
            ..CompileOptions::default()
        };
        let compiled = compile_cnf(engine, formula, &vtree, opts).ok()?;
        Some(engine.model_count(&compiled.diagram).ok()?.to_string())
    })
}
```

`ComponentPlan::Whole` compiles against the supplied vtree.
`CompileOptions::default()` instead detects components itself and compiles
each on a balanced vtree of its own. Each `CompileOptions` field's rustdoc
says what it turns on. The binary builds its vtree with
`vtree::build_vtree_with_components`, and passes the component trees of the
`VtreeBuild` it returns as `ComponentPlan::from_components`.

A diagram returned from the batch owns its storage and stays usable after the
batch ends.
