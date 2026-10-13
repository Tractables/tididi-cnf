# Weighted marginal levels

A weighted compile marginalizes the same levels as an unweighted one
([marginal-tdd-levels.md](marginal-tdd-levels.md)); the per-node values are
weights instead of counts. The library owns the representation:

| Topic | Library item |
|---|---|
| Where the values live, and how two diagrams' stores merge | `tididi::diagram::WeightStore` |
| The value type and its canonical exact form | `tididi::diagram::WeightValue` |
| The two arithmetic domains | `tididi::diagram::Arithmetic` |
| The pinned leaf column | invariant 11 in the library's [invariant table](https://github.com/Tractables/tididi/blob/main/docs/architecture.md#invariants) |
| Pair fusion on weighted levels | invariant 8 in the same table |

This page covers what the compiler decides: which domain a run gets, and how
to test a change.

## The domain

Each weighted mode fixes its domain: WMC counts in `Arithmetic::SignedLog`
(the answer carries a tolerance, and log arithmetic avoids the digit growth of
exact rationals; the constant `ARITHMETIC` in `src/stage/wmc.rs`), and PWMC
evaluates in exact rationals (the answer must be exact). A run is PWMC when it
is weighted and projected, however the projection was requested: `c t pwmc`,
`--mode pwmc`, or a `c p show` line. A weighted compile takes the domain with
its weights, in `compile::Output::WeightedCount`, so a library caller chooses
it there.

The run prints `[<mode>] arithmetic: exact rational` or
`… log (bounded)` on stderr. In the log domain the library skips pair fusion,
duplicate-run scaling and the equal-weight leaf ref rewrite, since `SignedLog`
equality is bit equality and log addition depends on order. A WMC run
therefore never reaches those rewrites. To run them on a real formula, add a
`c p show` line naming every variable: the run becomes PWMC in exact
arithmetic, and its answer is the plain weighted count. The projected route
builds its own vtree and preprocessing, so this tests the arithmetic, not a
WMC run's diagrams.

## Testing a change

`tests/weighted_marginalization.rs` checks the weighted marginalizing compile
against a plain compile evaluated under the same weights. Its ignored test
`fuzz_weighted_marginalize_vs_oracle` runs random CNFs under three weight
shapes; the module doc lists the environment variables that set the seed
range, formula size and shape. Run each shape, including one pass with 20 to
34 variables: smaller formulas rarely give the contraction passes anything to
rewrite.
