//! The model count's recovery: when a compile fails, the fallbacks, then a
//! Shannon split of the formula into cofactors that are counted the same way.
//!
//! ## Why a failed compile can be caught
//!
//! A refused allocation unwinds from its call site (`crate::mem` says why), so
//! `catch_unwind` here catches it. [`count_with_recovery`] runs every compile under
//! [`std::panic::catch_unwind`]. When it catches a panic, the state inside
//! the failed compile is by then either freed (drops ran) or unreachable.
//! The only live state is the input `CnfFormula` — which was passed by
//! reference from a caller frame that's still alive.
//!
//! ## What a failure is
//!
//! A compile fails when it panics, or when it gives up with the run's wall
//! still left (a memory refusal, or a deadline of its own). A compile that
//! gives up because the run's wall has passed ends the count: nothing after
//! it could answer in time.
//!
//! On the first compile's failure the caller's fallbacks run once, on the
//! whole formula, each under the same panic isolation. When none answers, the
//! formula is **Shannon-expanded** on a branching variable `x`:
//!
//! ```text
//!     count(F) = count(F ∧ x) + count(F ∧ ¬x)
//! ```
//!
//! and each cofactor is counted by the caller's cofactor compile, split again
//! on its failure. Each sub-formula gets a fresh compile attempt with a clean
//! allocator state (jemalloc's retained extents are still mapped, but the
//! live working set has been reset to zero). Any panic splits, not only a
//! refused allocation, so a logic bug splits too; each caught panic's message
//! and a memory-state snapshot go to stderr, so it stays visible in the log.
//! Each split strictly shrinks the formula's support, so the cascade ends
//! when every sub-formula compiles or the wall passes.
//!
//! ## Branching heuristic
//!
//! We pick the variable with the highest occurrence count across clauses, a
//! static count taken once per split: the variable most likely to trigger
//! unit propagation in the sub-CNF, giving both branches the largest
//! reduction.
//!
//! ## Per-cofactor work
//!
//! - **Each cofactor is prepared afresh** by the caller's cofactor compile:
//!   its support is strictly smaller than the formula's, so the formula's
//!   reduction and vtree no longer fit it.
//! - **The two branches share nothing.** count(F∧x) and count(F∧¬x) are
//!   separate compiles, even where they share structure.
//!
//! ## The wall
//!
//! The cascade asks whether the run's wall has passed before the first
//! compile, before the fallbacks, and before each Shannon cofactor, so the
//! split stops by it instead of grinding past into an external-timeout
//! `SIGKILL`. A post-deadline result is uncountable, so bailing only stops
//! dead-weight work.

use std::panic;
use std::time::Instant;

use num_bigint::BigUint;

use vitri::cnf::{Clause, CnfFormula, Literal, VarId};

use crate::budget::deadline_passed;
use crate::compile::CompileRun;
use crate::formula::{most_frequent_var, with_unit};
use crate::MIB;
use crate::mem::{mapped_bytes, purge_all_arenas, rlimit_as_bytes};
use crate::output::AnsweredBy;
use crate::stage::{Run, panic_payload_message};

/// A compile's answer: the count, in the space of the formula it was handed,
/// and the largest diagram it reached, in pairs.
pub(super) type Counted = (BigUint, usize);

/// What [`count_with_recovery`] answered: the count, the largest diagram any
/// of its compiles reached, the stage that produced the count, and the wall
/// from the first compile to the answer.
pub(super) struct Recovered {
    pub(super) count: BigUint,
    pub(super) peak_pairs: usize,
    pub(super) answered_by: AnsweredBy,
    pub(super) cascade_ms: u128,
}

/// Count `formula` by `first`, the first compile, and on its failure by the
/// recovery the module doc describes: `fallbacks`, when the caller has any,
/// then a Shannon split whose cofactors `cofactor` counts, on the run as a
/// cofactor's compile sees it. `first` and each fallback name the stage that
/// answered: the first compile is [`AnsweredBy::Compile`] unless the caller's
/// first call ran a stage of its own behind it. Peak sizes combine by `max`
/// across the split.
///
/// Prints the stage that answered and the wall it took. `None` when nothing
/// answered: the run's wall passed, or the split ran out of variables to
/// branch on.
pub(super) fn count_with_recovery(
    run: &Run<'_>,
    formula: &CnfFormula,
    first: impl FnOnce() -> Option<(Counted, AnsweredBy)>,
    fallbacks: Option<&dyn Fn() -> Option<(Counted, AnsweredBy)>>,
    cofactor: impl Fn(&Run<'_>, &CnfFormula) -> Option<Counted>,
) -> Option<Recovered> {
    let started = Instant::now();
    let answered = |(count, peak_pairs): Counted, answered_by: AnsweredBy| {
        let cascade_ms = started.elapsed().as_millis();
        eprintln!("[mc] answered by {} in {cascade_ms}ms", answered_by.token());
        Some(Recovered { count, peak_pairs, answered_by, cascade_ms })
    };
    if deadline_passed(run.deadline) {
        return None;
    }
    let trigger = match failure(run, first) {
        Ok((counted, answered_by)) => return answered(counted, answered_by),
        Err(trigger) => trigger?,
    };
    // The failed attempt may have burned the rest of the wall before it
    // died, so the wall is asked again before the fallbacks start.
    if let Some(fallbacks) = fallbacks
        && !deadline_passed(run.deadline)
    {
        // Purge the failed compile's pages so the fallbacks start from a
        // clean allocator state.
        log_and_purge_recovery(run.eng, "[mc] recovery", formula, 0, &format!("the fallbacks first, after: {trigger}"));
        if let Ok(Some((counted, answered_by))) = panic::catch_unwind(panic::AssertUnwindSafe(fallbacks)) {
            return answered(counted, answered_by);
        }
    }
    let cofactor_run = Run { compile: CompileRun { cofactor: true, ..run.compile }, ..*run };
    let counted = split(&cofactor_run, formula, 0, &[], &cofactor, &trigger)?;
    answered(counted, AnsweredBy::ShannonSplit)
}

/// `compile` under panic isolation: its answer, or why it failed — the panic's
/// message, or a give-up with the run's wall left — or `Err(None)` when it
/// gave up with the wall gone, which ends the count.
fn failure<T>(run: &Run<'_>, compile: impl FnOnce() -> Option<T>) -> Result<T, Option<String>> {
    match panic::catch_unwind(panic::AssertUnwindSafe(compile)) {
        Ok(Some(counted)) => Ok(counted),
        Ok(None) if deadline_passed(run.deadline) => Err(None),
        Ok(None) => Err(Some("the compile gave up with time left".to_owned())),
        Err(payload) => Err(Some(panic_payload_message(payload.as_ref()))),
    }
}

/// One cofactor at recovery `depth` counted: by `cofactor`, and on its
/// failure split again.
///
/// `branched` holds the variables the enclosing splits fixed.
fn count_or_split<F>(
    run: &Run<'_>,
    formula: &CnfFormula,
    depth: u32,
    branched: &[VarId],
    cofactor: &F,
) -> Option<Counted>
where
    F: Fn(&Run<'_>, &CnfFormula) -> Option<Counted>,
{
    // Past the deadline: stop before any more vtree-build / compile work.
    if deadline_passed(run.deadline) {
        return None;
    }
    match failure(run, || cofactor(run, formula)) {
        Ok(counted) => Some(counted),
        Err(trigger) => split(run, formula, depth, branched, cofactor, &trigger?),
    }
}

/// The split itself: branch on one variable, BCP-close both cofactors, count
/// each, and sum.
fn split<F>(
    run: &Run<'_>,
    formula: &CnfFormula,
    depth: u32,
    branched: &[VarId],
    cofactor: &F,
    trigger: &str,
) -> Option<Counted>
where
    F: Fn(&Run<'_>, &CnfFormula) -> Option<Counted>,
{
    // Log trigger + purge jemalloc pages from the just-failed compile.
    log_and_purge_recovery(run.eng, "[mc] shannon-split", formula, depth, &format!("split, after: {trigger}"));

    // Branch on the most frequent variable no enclosing split fixed, lowest
    // index on a tie, so every recovery level strictly shrinks the
    // unassigned-variable space.
    let mut ancestor = vec![false; formula.num_vars() as usize];
    for v in branched {
        ancestor[v.idx()] = true;
    }
    let Some(var) = most_frequent_var(formula, 0, |idx| !ancestor[idx]) else {
        eprintln!("[mc] shannon-split: no variable left to branch on; giving up");
        return None;
    };
    eprintln!("[mc] shannon-split: branching on var {} → 2 children", var.get());
    let mut child_branched: Vec<VarId> = Vec::with_capacity(branched.len() + 1);
    child_branched.extend_from_slice(branched);
    child_branched.push(var);

    // Both cofactors UNSAT ⇒ formula UNSAT ⇒ zero models.
    let mut acc: Counted = (BigUint::ZERO, 0);
    for (child_idx, unit) in [Literal::pos(var), Literal::neg(var)].into_iter().enumerate() {
        // Past the deadline the whole split can no longer produce a countable
        // result. Give the entire split up (None) rather than continue —
        // returning the partial `acc` would silently undercount. Checked before
        // any per-child work. Fires only once the wall has passed, so it never
        // abandons a countable result.
        if deadline_passed(run.deadline) {
            eprintln!("[mc] shannon-split: the time limit passed mid-split; giving up");
            return None;
        }
        let Some(sub) = simplify_via_unit(formula, unit) else {
            // UNSAT child — contributes 0. Skip the compile.
            eprintln!("[mc] shannon-split: child {}/2: UNSAT after BCP — 0 models", child_idx + 1);
            continue;
        };
        let (count, peak) = count_or_split(run, &sub, depth + 1, &child_branched, cofactor)?;
        // Purge between siblings so the next cofactor doesn't inherit this
        // one's retained extents.
        let _ = purge_all_arenas();

        acc.0 += count;
        acc.1 = acc.1.max(peak);
    }
    Some(acc)
}

/// Log the recovery trigger under `tag` and purge jemalloc pages.
///
/// Logs the recovery event to stderr, then
/// [`release_failed_compile`](crate::compile::release_failed_compile) gives
/// back the panicked parent's apply scratch and dirty pages before child
/// compiles begin allocating; the decay setting stays with the pressure check
/// in `crate::mem`.
///
/// It logs the [`HeapStats`](crate::mem::HeapStats) (`allocated` / `mapped` /
/// `retained`) before and after the purge, with the mallctl return code:
/// `allocated ≈ mapped` means live data the purge cannot reclaim, while
/// `allocated ≪ mapped + retained` means address space jemalloc holds, which
/// the purge gives back (under `retain:false` purged extents are unmapped).
fn log_and_purge_recovery(eng: &tididi::Engine, tag: &str, formula: &CnfFormula, depth: u32, what: &str) {
    let mapped = mapped_bytes();
    let limit = rlimit_as_bytes();
    eprintln!(
        "{tag}: {what} at depth {} ({} vars, {} clauses) \
         [mapped+retained={} MiB, rlimit_as={}]",
        depth,
        formula.num_vars(),
        formula.clauses().len(),
        mapped / MIB,
        limit.map_or_else(|| "unlimited".to_string(), |b| format!("{} MiB", b / MIB)),
    );
    // The heap counters, split, before → after the scratch
    // drop + purge, so the log keeps measuring the per-level pin (the before-
    // numbers) and shows the clean slate the children inherit (the after-
    // numbers); the `mapped + retained` sum alone is purge-invariant (see
    // mem.rs doc).
    let released = crate::compile::release_failed_compile(eng);
    let (held_before, held_after) = released.held_mib();
    let crate::compile::Released { before, after, purged } = released;
    // The mallctl return code: "Ok" or the bare errno int.
    let rc = match purged {
        Ok(()) => "Ok".to_string(),
        Err(e) => e.to_string(),
    };
    eprintln!(
        "{tag}: purge: rc={rc} allocated {}→{} MiB, mapped {}→{} MiB, \
         retained {}→{} MiB (mapped+retained {}→{})",
        before.allocated / MIB,
        after.allocated / MIB,
        before.mapped / MIB,
        after.mapped / MIB,
        before.retained / MIB,
        after.retained / MIB,
        held_before,
        held_after,
    );
}

/// Simplify `formula ∧ unit` via BCP closure, returning the simplified CNF
/// (or `None` if the conjunction is UNSAT).
///
/// Mechanism:
/// 1. Clone the original clauses and append `unit` as a unit clause.
/// 2. Ask vitri's unit-propagation operation for the fixpoint —
///    satisfied clauses drop, falsified literals shorten, derived units
///    cascade.
/// 3. If the closure produced an empty clause, the formula is UNSAT
///    under the unit assignment → return `None` (caller skips the child,
///    it contributes 0 models).
/// 4. Append the (post-propagation) forced literals back as unit clauses
///    so the residual formula still constrains them — this preserves
///    `num_vars` semantics: model count over `[num_vars]` is unchanged
///    by the BCP rewrite (the unit clauses pin each forced var to one
///    polarity ×1 rather than ×2).
///
/// Propagating rather than only appending the unit means downstream
/// component detection and vtree construction see the *real* residual
/// occurrence graph: a clause `(x ∨ y ∨ z)` with `x` forced true is
/// dropped entirely, not kept verbatim; a binary `(x ∨ y)` with `x`
/// forced false collapses to a derived unit `(y)` that may cascade
/// further. On dense recovery sub-formulas, this can shrink support
/// significantly even when the surface var count is unchanged.
fn simplify_via_unit(formula: &CnfFormula, unit: Literal) -> Option<CnfFormula> {
    let seeded = with_unit(formula, unit);
    let vitri::cnf::UnitPropagation { residual, forced } = vitri::cnf::propagate_units(&seeded);
    let mut clauses = residual.into_clauses();
    if clauses.iter().any(|clause| clause.literals.is_empty()) {
        return None;
    }
    // Re-pin every BCP-derived forced literal as a unit clause so the
    // residual CNF still binds the variable (preserves num_vars semantics).
    for lit in &forced {
        clauses.push(Clause::new(vec![*lit]));
    }
    Some(
        CnfFormula::new(formula.num_vars(), clauses)
            .expect("propagation names no variable the seeded formula did not"),
    )
}

#[cfg(test)]
#[path = "tests/recovery.rs"]
mod tests;
