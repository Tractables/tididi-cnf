//! The compile attempts the model count runs, and how each is built.
//!
//! One attempt is one formula plus the per-component vtrees it will be
//! compiled on. The primary compile, the no-SBVA retry and a Shannon
//! cofactor's compile take the tree vitri built beside the reduction each is
//! made of; the unreduced retry, the stall recompile and a cofactor compiled
//! as it stands have no such pairing and build their own trees through
//! [`portfolio_attempt`], bounded by the time limit the attempt will be
//! compiled against, then hand them to the same constructor. All end at the same `CompileAttempt`, so an attempt cannot
//! arrive at the compile half-built. [`count_attempt`] compiles one under
//! panic isolation.

use std::time::Instant;

use num_bigint::BigUint;
use vitri::cnf::CnfFormula;

use crate::budget::vtree_deadline;
use crate::compile::model_count_streaming;
use crate::stage::{PORTFOLIO_VTREE_SPEC, Run, run_isolated};
use crate::vtree::{
    ComponentVtree, VtreeBuild, VtreeOptions, build_vtree_with_components, built_or_give_up, whole_formula_component,
};

/// One attempt of a streaming model-count compile: a formula, and its
/// components with their vtrees, pre-built by the caller.
pub(super) struct CompileAttempt {
    /// The formula this attempt compiles.
    pub(super) formula: CnfFormula,
    /// The formula's components and their vtrees, in the order the compile
    /// drains them.
    pub(super) components: Vec<ComponentVtree>,
    /// `2^k` lift from this attempt's formula back to the base formula's
    /// space, which [`count_attempt`] folds into the count it returns (the
    /// correction invariant, in the `mc` module doc). The unreduced retry and
    /// a cofactor compiled as it stands carry 0: each compiles its formula
    /// directly.
    pub(super) lift_pow2: u32,
}

/// Compile `attempt`: `model_count_streaming` over its pre-built vtrees,
/// under panic isolation, with what the run sets for it, against `deadline`.
/// Returns `(count, peak_size)`, the count lifted by the attempt's
/// `lift_pow2`, or `None` if the compile gave up or panicked; a panic is
/// logged as `[mc] compile: panicked: ...`.
pub(super) fn count_attempt(
    run: &Run<'_>,
    attempt: CompileAttempt,
    deadline: Option<Instant>,
) -> Option<(BigUint, usize)> {
    let CompileAttempt { formula, components, lift_pow2 } = attempt;
    let (components, vtrees): (Vec<_>, Vec<_>) = components.into_iter().map(|cv| (cv.component, cv.vtree)).unzip();
    // The compile asks for one vtree per component, in order.
    let mut vtrees = vtrees.into_iter();
    run_isolated("[mc] compile", || {
        model_count_streaming(run.eng, &formula, &components, |_sub| vtrees.next(), None, deadline, run.compile)
    })
    .map(|(count, peak)| (count << lift_pow2, peak))
}

/// Consume a vitri frontend attempt's already-built vtree into one compile
/// attempt.
///
/// This is the only unpacking path for paired whole-run frontend results. The
/// primary and checkpointed retry attempts must use the tree vitri built over
/// their own reduced formula; rebuilding here would separate the two halves of
/// vitri's result and repeat construction work.
pub(super) fn attempt_from_vitri_build(
    formula: CnfFormula,
    built: VtreeBuild,
    lift_pow2: u32,
) -> CompileAttempt {
    let components = built.components.unwrap_or_else(|| {
        vec![ComponentVtree { vtree: built.root, component: whole_formula_component(&formula) }]
    });
    CompileAttempt { formula, components, lift_pow2 }
}

/// An attempt for `formula` on vitri's portfolio, every component's
/// vtree built up front by vitri, with no Arjun lift: the unreduced retry's,
/// a cofactor's compiled as it stands, and the stall recompile's, whose caller
/// sets the lift of the reduction it compiles. The portfolio is vitri's default, or the one catalog entry
/// `only_entry` names. `None` when the construction fails, as
/// [`built_or_give_up`] reads it.
///
/// `deadline` is the wall the attempt will be compiled against, and its vtree
/// construction respects it too: an attempt that spends its window building a
/// tree defeats the window it was given.
pub(super) fn portfolio_attempt(
    run: &Run<'_>,
    formula: &CnfFormula,
    deadline: Option<Instant>,
    only_entry: Option<&'static str>,
) -> Option<CompileAttempt> {
    let ctx = VtreeOptions { only_portfolio_entry: only_entry, ..VtreeOptions::default() }
        .with_deadline(vtree_deadline(deadline))
        .with_run_policy(run.config);
    let built = run.config.vtree_time.time(|| build_vtree_with_components(formula, PORTFOLIO_VTREE_SPEC, &ctx));
    let built = built_or_give_up(built)?.bind(run.eng);
    Some(attempt_from_vitri_build(formula.clone(), built, 0))
}

#[cfg(test)]
#[path = "tests/attempt.rs"]
mod tests;
