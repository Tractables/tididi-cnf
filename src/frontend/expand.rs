//! Reconstruct a compiled Boolean function in its original variable space.
//!
//! vitri's compile record partitions originals into signed alias classes,
//! constants and free variables, which
//! [`original_expansion`](super::record::original_expansion) states as the
//! library's [`VariableExpansion`]. The library's [`Engine::expand_variables`]
//! replaces each reduced leaf by its whole signed class and grafts the
//! constants, as one cube, and the free variables on above the root. This
//! reconstructs the function itself, so no count lift is owed after expansion.

use std::sync::Arc;

use tididi::diagram::Tdd;
use tididi::restructure::{ExpandError, VariableExpansion};
use tididi::vtree::Vtree;
use tididi::{Engine, OperationError};

/// Expand one reduced TDD into the original variables, on the vtree it ends
/// on, which the compile may have rotated away from the vtree it started on.
///
/// A total original map names every original exactly once and gives every
/// reduced variable a class, so only a refusal of the work itself, an armed
/// stop or a refused allocation in the compile's bounded batch, can come back.
pub(crate) fn expand_to_originals(
    eng: &Engine,
    tdd: &Tdd,
    expansion: &VariableExpansion,
) -> Result<Tdd, OperationError> {
    eng.expand_variables(tdd, expansion).map_err(|error| match error {
        ExpandError::Operation(error) => error,
        error => panic!("a total original map names each original once: {error}"),
    })
}

/// Materialize the conjunction described by a compile record with no reduced
/// variables. Every original is then either constant or free.
///
/// vitri reports a run fully resolved exactly when the reduced formula has no
/// variables, so there is no class, and it refuses a formula with none before
/// it preprocesses, so the balanced vtree over the originals has a leaf.
pub(crate) fn fully_resolved_tdd(eng: &Engine, expansion: &VariableExpansion) -> Result<Tdd, OperationError> {
    let vtree = Arc::new(Vtree::balanced(expansion.num_vars));
    // With no class left, `original_expansion` put every original among the
    // constants or the free variables. The free variables carry no literal: a
    // cube says nothing about them, which is exactly the One leaf this
    // expansion places for them.
    eng.cube(&vtree, expansion.constants.iter().copied())
}

#[cfg(test)]
#[path = "tests/expand.rs"]
mod tests;
