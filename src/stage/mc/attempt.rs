//! The compile attempts the model count runs, and how each is built.
//!
//! One attempt is one formula plus the per-component vtrees it will be
//! compiled on. The primary compile takes the tree Vitri already built beside
//! the reduction it is made of; the unreduced retry and a Shannon cofactor have
//! no such pairing and build their own trees through [`portfolio_attempt`],
//! bounded by the time limit the attempt will be compiled against, then hand
//! them to the same constructor. All end at the same `CompileAttempt`, so an
//! attempt cannot arrive at the compile half-built. [`count_attempt`] compiles
//! one on the calling thread under panic isolation.
use std::sync::Arc;
use std::time::Instant;

use num_bigint::BigUint;
use tididi::vtree::Vtree;
use vitri::cnf::CnfFormula;

use crate::budget::vtree_deadline;
use crate::compile::model_count_streaming;
use crate::stage::{PORTFOLIO_VTREE_SPEC, Run, run_isolated};
use crate::vtree::{
    Component, VtreeBuild, VtreeOptions, build_vtree_with_components, built_or_give_up, whole_formula_component,
};

/// One attempt of a streaming model-count compile: a formula, its components
/// and their vtrees, pre-built by the caller.
pub(super) struct CompileAttempt {
    /// The formula this attempt compiles.
    pub formula: CnfFormula,
    /// The formula's components, in the order the compile drains them.
    pub components: Vec<Component>,
    /// One vtree per entry of `components`, in the same order.
    pub vtrees: Vec<Arc<Vtree>>,
    /// `2^k` lift from this attempt's formula back to the base formula's
    /// space, which [`count_attempt`] folds into the count it returns (the
    /// correction invariant, in the `mc` module doc). Shannon cofactors and
    /// the unreduced retry carry 0: they compile the base formula, or a
    /// cofactor of it, directly.
    pub lift_pow2: u32,
}

/// Compile `attempt` on the calling thread: `model_count_streaming`
/// over its pre-built vtrees, under panic isolation, with what the run sets
/// for it, against `deadline`. Returns `(count, peak_size)`, the count lifted
/// by the attempt's `lift_pow2`, or `None` if the compile gave up or panicked;
/// a panic is logged as `[mc] compile: panicked: ...`.
pub(super) fn count_attempt(
    run: &Run<'_>,
    attempt: CompileAttempt,
    deadline: Option<Instant>,
) -> Option<(BigUint, usize)> {
    let CompileAttempt { formula, components, vtrees, lift_pow2 } = attempt;
    let mut prebuilt = vtrees.into_iter();
    run_isolated("[mc] compile", || {
        model_count_streaming(
            run.eng,
            &formula,
            &components,
            |_sub| {
                Some(prebuilt.next()
                    .expect("pre-built vtree drained: vtrees.len() must match components.len()"))
            },
            None,
            deadline,
            run.compile,
        )
    })
    .map(|(count, peak)| (count << lift_pow2, peak))
}

/// Consume a Vitri frontend attempt's already-built vtree into one compile
/// attempt.
///
/// This is the only unpacking path for paired whole-run frontend results.  The
/// primary and checkpointed retry attempts must use the tree Vitri built over
/// their own reduced formula; rebuilding here would separate the two halves of
/// Vitri's result and repeat construction work.
pub(super) fn attempt_from_vitri_build(
    formula: CnfFormula,
    built: VtreeBuild,
    lift_pow2: u32,
) -> CompileAttempt {
    let (components, vtrees): (Vec<_>, Vec<_>) = match built.components {
        Some(cvs) => cvs.into_iter().map(|cv| (cv.component, cv.vtree)).unzip(),
        None => (vec![whole_formula_component(&formula)], vec![built.root]),
    };
    CompileAttempt {
        formula,
        components,
        vtrees,
        lift_pow2,
    }
}

/// An attempt for `formula` on Vitri's portfolio, every component's
/// vtree built up front by Vitri, with no Arjun lift: the unreduced retry's and
/// a Shannon cofactor's. `None` when the construction fails, as
/// [`built_or_give_up`] reads it.
///
/// `deadline` is the wall the attempt will be compiled against, and its vtree
/// construction respects it too: an attempt that spends its window building a
/// tree defeats the window it was given.
pub(super) fn portfolio_attempt(
    run: &Run<'_>,
    formula: &CnfFormula,
    deadline: Option<Instant>,
) -> Option<CompileAttempt> {
    let ctx = VtreeOptions::default()
        .with_deadline(vtree_deadline(deadline))
        .with_run_policy(run.config);
    let built = built_or_give_up(build_vtree_with_components(formula, PORTFOLIO_VTREE_SPEC, &ctx))?.bind(run.eng);
    Some(attempt_from_vitri_build(formula.clone(), built, 0))
}

#[cfg(test)]
#[path = "tests/attempt.rs"]
mod tests;
