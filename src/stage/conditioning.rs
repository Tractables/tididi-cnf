//! Show-variable conditioning for the projected count, and the projected
//! compile it conditions down to. `condition_on_show` is the entry
//! `compile_projected` in `pmc.rs` falls back to; `condition_and_count` splits
//! a formula on show variables until each branch compiles within its time
//! limit, and sums the branch counts; `pmc_branch_count` compiles one branch
//! through `projected_compile`, the compile every PMC compile runs.

use std::sync::Arc;
use std::time::Instant;

use tididi::vtree::Vtree;
use vitri::cnf::CnfFormula;

use super::Run;
use super::projected::{CountVtree, Step, count_vtree, projection_step};
use crate::vtree::VtreeOptions;
use crate::problem::{Problem, ShowSet, Unweighted};

/// Dynamic per-branch cutset pick: unit-propagate the conditioned `formula`
/// (count-preserving; used here only for scoring) and return the show var with
/// the highest occurrence in the propagated residual's clauses of length >= 2.
/// Returns `None` when no show var occurs in such a clause (the residual is
/// decided over the show set → the caller should compile directly).
///
/// Fixing a hub variable unit-propagates chains that collapse whole regions, so
/// variables that were low-occurrence in the original formula can become the
/// residual's true hubs. Tie-break by lowest var id. Sound regardless of the
/// pick: `F = F|v + F|¬v` holds for any `v`, so a different choice only changes
/// which branches compile in budget, never the count.
fn pick_dynamic_branch_var(formula: &CnfFormula, show: &ShowSet) -> Option<vitri::cnf::VarId> {
    let residual = vitri::cnf::propagate_units(formula).residual;
    crate::formula::most_frequent_var(&residual, 2, |idx| show.contains(vitri::cnf::VarId::from_idx(idx)))
}

/// How deep `condition_and_count` splits and what each leaf compile gets.
struct ConditioningLimits {
    /// Below this depth, split without trying a compile first.
    min_depth: usize,
    /// Deepest split; at this depth the leaf is compiled as it is.
    depth_cap: usize,
    /// Wall for each leaf compile.
    branch_ms: u64,
    /// Arjun window for each leaf's re-reduction (0 skips it).
    arjun_ms: u64,
    /// The run's wall. No leaf compiles past it, and once it has passed the
    /// recursion gives up without compiling.
    deadline: Option<Instant>,
}

/// The projected count of `formula` over `show`, by show-var conditioning.
///
/// From `min_depth` on, tries to compile this branch under its own
/// `branch_ms` wall; below it, or when that compile gives up, splits on the
/// show var `pick_dynamic_branch_var` picks, recurses on both polarities and
/// sums. Sound because show vars are counted:
///   Σ_X ∃_Y F = Σ over show-cutset assignments of (Σ_X ∃_Y F | assignment).
/// At `depth_cap`, or when no show var is left to split on, the branch gets
/// one last compile under the same wall. `None` if that fails too, or once
/// the run's wall, `cfg.deadline`, has passed.
fn condition_and_count(
    run: &Run<'_>,
    formula: &CnfFormula,
    show: &ShowSet,
    depth: usize,
    cfg: &ConditioningLimits,
) -> Option<num_bigint::BigUint> {
    if crate::budget::deadline_passed(cfg.deadline) {
        return None;
    }
    let leaf = |formula: &CnfFormula| {
        // `pmc_branch_count` installs this deadline in the engine's apply
        // limits: the leaf's own wall, cut at the run's.
        let branch_wall = Instant::now() + std::time::Duration::from_millis(cfg.branch_ms);
        let deadline = Some(crate::budget::not_past(branch_wall, cfg.deadline));
        pmc_branch_count(run, formula, show, deadline, cfg.arjun_ms)
    };
    // Below `min_depth`, split unconditionally, without attempting a compile.
    // Shallow compiles of a dense, compute-bound projected component balloon
    // before any recovery trigger fires, so one-level-at-a-time adaptive descent
    // crawls or OOMs. So this conditions a depth-K cutset up front (2^K
    // already-small branches, each with its own rebuilt vtree) and deepens
    // adaptively only on the residual hard branches.
    if depth >= cfg.min_depth
        && let Some(count) = leaf(formula)
    {
        return Some(count);
    }
    if depth >= cfg.depth_cap {
        return leaf(formula);
    }
    let Some(v) = pick_dynamic_branch_var(formula, show) else {
        return leaf(formula);
    };
    // Conditioning appends the unit without propagating, so variable ids and
    // `num_vars` stay put and the show set stays keyed to them.
    let branch = |lit| condition_and_count(run, &crate::formula::with_unit(formula, lit), show, depth + 1, cfg);
    let a = branch(vitri::cnf::Literal::pos(v))?;
    let b = branch(vitri::cnf::Literal::neg(v))?;
    Some(a + b)
}

/// The projected compile every PMC compile runs: marginalize, ∃-forget
/// `forget_set` during the compile, cuttable mid-apply at `deadline`, under
/// what the run sets for each compile; then its projected count
/// ([`Compiled::projected_count`](crate::compile::Compiled::projected_count)).
///
/// `None` means the compile gave up, cut by the deadline or refused an apply
/// by the batch limits (a give-up never yields part of a diagram), or the
/// limits refused the readout.
pub(super) fn projected_compile(
    run: &Run<'_>,
    work: &CnfFormula,
    vtree: &Arc<Vtree>,
    components: crate::compile::ComponentPlan<'_>,
    forget_set: &[tididi::vtree::VarId],
    deadline: Option<Instant>,
) -> Option<num_bigint::BigUint> {
    let compiled = crate::compile::settle(crate::compile::compile_with(
        run.eng,
        work,
        vtree,
        crate::compile::CompileOptions {
            plan: components,
            output: crate::compile::Output::ProjectedCount(forget_set),
            deadline,
        },
        run.compile,
    ))?;
    compiled.projected_count(run.eng).ok()
}

/// One projected compile of `formula`, rebuilding the vtree on this
/// (conditioned) formula. Returns the branch's projected model count, lifted
/// by the re-reduction's lift, or `None` if the compile trips `deadline`.
///
/// The vtree is built from Vitri's portfolio, not the run's `--vtree` spec:
/// conditioning does many per-branch builds, so it needs a cheap-to-build vtree,
/// and the run's spec may be an expensive one.
///
/// Why rebuild the vtree per branch (not reuse the parent's): conditioning the
/// hub show vars changes the formula's structure graph; the parent vtree,
/// optimized for the UN-conditioned formula, drives a giant single-level scatter
/// that balloons past any byte budget without the mid-batch check catching it
/// (one scatter == one merge iteration). With `arjun_ms > 0` the branch is
/// re-reduced first and compiled in the reduction's numbering, with its show
/// set and lift.
///
/// The compile gives up cleanly on `deadline` rather than panicking.
fn pmc_branch_count(
    run: &Run<'_>,
    formula: &CnfFormula,
    show: &ShowSet,
    deadline: Option<Instant>,
    arjun_ms: u64,
) -> Option<num_bigint::BigUint> {
    let input = Problem::<Unweighted> { formula: formula.clone(), show: Some(show.clone()), weights: (), lift: 0 };

    // Re-run projected Arjun inside a conditioned branch. This is deliberately
    // an Arjun-only Vitri bundle: the branch's own compile performs projection,
    // and `KeepSound` accepts any correctly mapped checkpoint, including one
    // with no projection gain.
    // The local reduced numbering never leaves this compile attempt.
    let exact_arjun = crate::budget::clamped_to_deadline_ms(arjun_ms, deadline, Instant::now())
        .map(std::time::Duration::from_millis);
    let branch = match exact_arjun {
        Some(exact_arjun) => {
            let preprocess_config = crate::frontend::request::projection_stage(
                run.config,
                vitri::cnf::Mode::Pmc,
                deadline,
                Some(exact_arjun),
                vitri::config::ProjectionPolicy::ArjunOnly(vitri::config::ProjectionNoGain::KeepSound),
            );
            match projection_step(&input, None, &preprocess_config, "[pmc] condition", "the branch input") {
                Step::Answer(zero) => return Some(zero),
                Step::Rewrite(reduced) => reduced,
                Step::Decline => input,
            }
        }
        None => input,
    };
    let cformula = &branch.formula;
    let cshow = branch.show.clone().unwrap_or_else(ShowSet::empty);
    let inner_shift = branch.lift as usize;

    // Trivially-SAT branch: conditioning + re-Arjun can collapse a branch to an
    // empty clause-set (every assignment satisfies). `compile_cnf` returns the
    // constant one for a clause-less formula without forgetting anything, and
    // Vitri builds no vtree over a formula with no variables, so count it
    // directly: ∃-forget the hidden vars, every show var is free →
    // 2^|show| << inner_shift.
    if cformula.clauses().is_empty() {
        return Some((num_bigint::BigUint::from(1u32) << cshow.len()) << inner_shift);
    }

    // Peak selection, as `counting_vtree` picks for a run with a show set:
    // conditioning only runs on one. This branch's own deadline is the
    // tightest wall that applies — the conditioning cascade builds one vtree
    // per branch, so an unbounded build here multiplies across the whole
    // branch set.
    let CountVtree { vtree, component_vtrees, projected } = count_vtree(
        run,
        cformula,
        super::PORTFOLIO_VTREE_SPEC,
        &VtreeOptions::by_peak_width().with_deadline(crate::budget::vtree_deadline(deadline)),
        Some(&cshow),
    )?;
    // The per-branch deadline, armed on the engine around the compile and the
    // count read off it: the compile arms it around itself alone, and the
    // readout after it is cut at it too. A cut branch gives up (`None`), and
    // the caller splits it deeper.
    let _deadline = crate::budget::deadline_scope(run.eng, deadline);
    let count = projected_compile(
        run,
        cformula,
        &vtree,
        crate::compile::ComponentPlan::from_components(component_vtrees.as_deref()),
        &projected,
        deadline,
    )?;
    Some(count << inner_shift)
}

/// Reactive projected-conditioning recovery: `condition_and_count` from depth 0,
/// splitting up to 12 show vars deep (4 before any compile is tried), each
/// branch on a 30 s wall with a 10 s re-Arjun, none past the run's wall.
/// Invoked only after the single-shot projected compile gives up or panics, so
/// a compile that finishes is never conditioned. Returns the projected count over
/// `show` in `work`'s numbering, or `None` if a branch still fails at the
/// deepest split.
///
/// Also `None` for an empty show set: with no show variable to split on, the
/// one branch would be another whole compile, which the last compile behind
/// this stage already is.
pub(super) fn condition_on_show(
    run: &Run<'_>,
    work: &CnfFormula,
    show: &ShowSet,
) -> Option<num_bigint::BigUint> {
    if show.is_empty() {
        return None;
    }
    let depth_cap = 12.min(crate::enumeration::constrained_show_count(work, show));
    let cfg = ConditioningLimits {
        min_depth: 4.min(depth_cap),
        depth_cap,
        branch_ms: 30_000,
        arjun_ms: 10_000,
        deadline: run.deadline,
    };
    condition_and_count(run, work, show, 0, &cfg)
}

#[cfg(test)]
#[path = "tests/conditioning.rs"]
mod tests;
