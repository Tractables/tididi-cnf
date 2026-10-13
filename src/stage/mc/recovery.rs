//! The model count's recovery: when a compile fails, the fallbacks, then a
//! Shannon split of the formula into cofactors that are counted the same way.
//!
//! ## Why a failed compile can be caught
//!
//! When the compiler runs out of memory deep in the recursive cascade,
//! jemalloc returns null and Rust's default `__rust_alloc_error_handler`
//! (Rust 1.81+) panics with `"memory allocation of N bytes failed"`.
//! That panic originates at the call site of the allocation
//! (`Vec::with_capacity`, `Box::new`, `RawVec::reserve`, …) — *not* from
//! inside `GlobalAlloc::alloc`. That distinction matters because
//! `__rust_alloc` is `#[rustc_nounwind]`; unwinding from inside it is UB
//! and was empirically observed to terminate with rc=101 *without* ever
//! reaching `catch_unwind`. Letting the default handler do the panic from
//! a regular (unwindable) frame restores the unwind path.
//!
//! [`count_with_recovery`] runs every compile under
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
//! - **Arjun is re-run** on the cofactors of the first `SPLIT_ARJUN_MAX_DEPTH`
//!   split levels; deeper cofactors compile as they stand.
//! - **Vtree is rebuilt** from the sub-CNF by the caller's cofactor compile;
//!   that's necessary because the support of the sub-CNF is strictly
//!   smaller and the original vtree may reference variables that no
//!   longer constrain anything.
//! - **The two branches share nothing.** count(F∧x) and count(F∧¬x) are
//!   separate compiles, even where they share structure.
//!
//! ## The wall
//!
//! The cascade asks whether the run's wall has passed before the first
//! compile, before the fallbacks, and before each Shannon cofactor, so the
//! split, and its per-child re-Arjun in `prepare_cofactor`, stop by it instead
//! of grinding past into an external-timeout `SIGKILL`. A post-deadline result
//! is uncountable, so bailing only stops dead-weight work. It bounds the
//! re-Arjun both ways: whether a cofactor's re-Arjun starts (the check before
//! each child) and how long it may then run
//! ([`crate::budget::clamped_to_deadline_ms`] in `prepare_cofactor`). With only
//! the first, a child starting a millisecond before the deadline still takes
//! the full policy budget of up to 300 s, which is wall the stage below it had
//! been promised.

use std::panic;
use std::time::Instant;

use num_bigint::BigUint;

use vitri::cnf::{Clause, CnfFormula, Literal, VarId};

use crate::budget::{clamped_to_deadline_ms, deadline_passed, not_past, resolve_scaled};
use crate::compile::CompileRun;
use crate::formula::{most_frequent_var, with_unit};
use crate::frontend::request::arjun_stage;
use crate::mem::{
    HeapStats, mapped_bytes, purge_all_arenas, reclaim_eagerly, rlimit_as_bytes,
};
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
/// each naming the stage that answered, then a Shannon split whose cofactors
/// `cofactor` counts, on the run as a cofactor's compile sees it. Peak sizes
/// combine by `max` across the split.
///
/// Prints the stage that answered and the wall it took. `None` when nothing
/// answered: the run's wall passed, or the split ran out of variables to
/// branch on.
pub(super) fn count_with_recovery(
    run: &Run<'_>,
    formula: &CnfFormula,
    first: impl FnOnce() -> Option<Counted>,
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
        Ok(counted) => return answered(counted, AnsweredBy::Compile),
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
fn failure(run: &Run<'_>, compile: impl FnOnce() -> Option<Counted>) -> Result<Counted, Option<String>> {
    match panic::catch_unwind(panic::AssertUnwindSafe(compile)) {
        Ok(Some(counted)) => Ok(counted),
        Ok(None) if deadline_passed(run.deadline) => Err(None),
        Ok(None) => Err(Some("the compile gave up with time left".to_owned())),
        Err(payload) => Err(Some(panic_payload_message(payload.as_ref()))),
    }
}

/// A post-BCP Shannon cofactor, prepared for its count.
enum Cofactor {
    /// The re-reduction resolved every variable: the count is `2^lift_pow2`,
    /// with nothing to compile.
    Resolved { lift_pow2: u32 },
    /// The re-reduction's formula, numbered afresh, whose count lifts by
    /// `2^lift_pow2`.
    Reduced { formula: CnfFormula, lift_pow2: u32 },
    /// The cofactor as BCP left it, in the numbering of the formula it came
    /// from.
    Unreduced(CnfFormula),
}

/// One cofactor at recovery `depth` counted: by `cofactor`, and on its
/// failure split again.
///
/// `branched` holds the variables the enclosing splits fixed since this
/// formula's numbering began: the whole formula's, or the re-reduction that
/// renumbered it.
fn count_cofactor<F>(
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

/// The split itself: branch on one variable, BCP-close both cofactors, re-Arjun
/// each at shallow depth, count each, and sum.
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
    eprintln!("[mc] shannon-split: branching on var {} → 2 children", var.idx());
    let mut child_branched: Vec<VarId> = Vec::with_capacity(branched.len() + 1);
    child_branched.extend_from_slice(branched);
    child_branched.push(var);

    // Both cofactors UNSAT ⇒ formula UNSAT ⇒ zero models.
    let mut acc: Counted = (BigUint::ZERO, 0);
    for (child_idx, unit) in [Literal::pos(var), Literal::neg(var)].into_iter().enumerate() {
        // Past the deadline the whole split can no longer produce a countable
        // result, and the per-child `prepare_cofactor` re-Arjun can burn up to
        // 300s. Give the entire split up (None) rather than continue — returning
        // the partial `acc` would silently undercount. Checked before any
        // per-child work (BCP / re-Arjun / recurse). Fires only once the wall
        // has passed, so it never abandons a countable result.
        if deadline_passed(run.deadline) {
            eprintln!("[mc] shannon-split: the time limit passed mid-split; giving up");
            return None;
        }
        let Some(sub) = simplify_via_unit(formula, unit) else {
            // UNSAT child — contributes 0. Skip the compile.
            eprintln!("[mc] shannon-split: child {}/2: UNSAT after BCP — 0 models", child_idx + 1);
            continue;
        };

        let (count, peak) = match prepare_cofactor(run, sub, depth, child_idx) {
            Cofactor::Resolved { lift_pow2 } => (BigUint::from(1u32) << lift_pow2, 0),
            // A reduction renumbers the cofactor, and the variables the
            // enclosing splits fixed are gone from it, so none is excluded.
            Cofactor::Reduced { formula, lift_pow2 } => {
                let (count, peak) = count_cofactor(run, &formula, depth + 1, &[], cofactor)?;
                (count << lift_pow2, peak)
            }
            Cofactor::Unreduced(formula) => count_cofactor(run, &formula, depth + 1, &child_branched, cofactor)?,
        };
        // Purge between siblings so the next cofactor doesn't inherit this
        // one's retained extents.
        let _ = purge_all_arenas();

        acc.0 += count;
        acc.1 = acc.1.max(peak);
    }
    Some(acc)
}

/// Absolute budget for a recovery cofactor's Arjun pass when the enclosing run
/// declares no wall.
const RECOVERY_ARJUN_ABS_DEFAULT_MS: u64 = 60_000;

fn prepare_cofactor(run: &Run<'_>, sub: CnfFormula, depth: u32, child_idx: usize) -> Cofactor {
    let deadline = run.deadline;
    // Re-run Arjun on the BCP-closed sub-formula. Sampling-set shrinkage
    // compounds across recovery levels; gate by depth to bound the
    // exponential cost of 2^N budgeted Arjun calls. The returned multiplier
    // is applied to this child's count before the sum.
    // 10% of max-time, clamped to [5 s, 300 s] and to the time left before
    // the cascade's deadline (see the section comment above). `None` is a budget with
    // nothing left in it, which compiles the cofactor as it stands, as the
    // depth gate does.
    let now = std::time::Instant::now();
    let budget_ms = (depth < SPLIT_ARJUN_MAX_DEPTH)
        .then(|| {
            clamped_to_deadline_ms(
                resolve_scaled(
                    run.config.time_limit_ms,
                    0.10,
                    RECOVERY_ARJUN_ABS_DEFAULT_MS,
                    5_000,
                    300_000,
                ),
                deadline,
                now,
            )
        })
        .flatten();
    let Some(budget_ms) = budget_ms else {
        return Cofactor::Unreduced(sub);
    };
    eprintln!(
        "[mc] shannon-split: child {}/2: re-Arjun budget {}ms on {}v/{}c",
        child_idx + 1,
        budget_ms,
        sub.num_vars(),
        sub.clauses().len(),
    );
    // `budget_ms` is already the recovery allowance, cut to the run's
    // wall: Vitri takes it as given, and stops there.
    let budget = std::time::Duration::from_millis(budget_ms);
    let reduce_config = arjun_stage(
        run.config,
        vitri::cnf::Mode::Mc,
        Some(not_past(now + budget, deadline)),
        budget,
        vitri::config::ArjunClauseGrowth::default(),
    );
    match vitri::bundle::preprocess(
        &sub,
        &vitri::cnf::CnfMeta::default(),
        &reduce_config,
    ) {
        // Only a kept Arjun reduction changes the post-BCP cofactor;
        // every other outcome, a preprocessing error included, keeps
        // the raw cofactor.
        Ok(ar) if matches!(ar.stages.arjun, Some(vitri::bundle::StageOutcome::Ran)) => {
            let arjun_pow2 = ar.count_lift.arjun_pow2;
            eprintln!(
                "[mc] shannon-split: child {}/2: arjun {}v/{}c → {}v/{}c, lift=2^{}",
                child_idx + 1,
                sub.num_vars(),
                sub.clauses().len(),
                ar.reduced.num_vars(),
                ar.reduced.clauses().len(),
                arjun_pow2,
            );
            check_arjun_accounting(&sub, &ar, depth, child_idx);
            if ar.reduced.num_vars() == 0 {
                eprintln!(
                    "[mc] shannon-split: child {}/2: arjun resolved every variable — count = 2^{}",
                    child_idx + 1,
                    arjun_pow2,
                );
                return Cofactor::Resolved { lift_pow2: arjun_pow2 };
            }
            Cofactor::Reduced { formula: ar.reduced, lift_pow2: arjun_pow2 }
        }
        Ok(ar) => {
            eprintln!(
                "[mc] shannon-split: child {}/2: arjun did not yield a kept reduction ({:?}) — \
                 using post-BCP sub-formula",
                child_idx + 1,
                ar.stages.arjun,
            );
            Cofactor::Unreduced(sub)
        }
        Err(error) => {
            eprintln!(
                "[mc] shannon-split: child {}/2: preprocessing failed ({error}) — \
                 using post-BCP sub-formula",
                child_idx + 1,
            );
            Cofactor::Unreduced(sub)
        }
    }
}

/// Soundness guard (A) on a re-Arjun'd cofactor: free-eliminated vars (each ×2)
/// must be a subset of the vars Arjun removed. BCP and Shannon-fixing preserve
/// `num_vars`, so Arjun is the only place it moves; this local inequality at
/// every re-Arjun is, by induction, the full per-path variable accounting.
///
/// Arjun also adds variables (bounded variable addition), and an added one was
/// never in `sub` to be removed — so the count of removals is
/// `sub.num_vars() + added - ar.reduced.num_vars()`, not
/// `sub.num_vars() - ar.reduced.num_vars()`, and stating the guard without the
/// `added` term asserts a reduction never grew. It does: the pipeline's own
/// reduction can add variables, and the guard without the `added` term fires on
/// a sound reduction. `added` is exactly the reduced variables with no
/// input-space name (`PreprocessRecord::reduced_to_original_dimacs`), which is
/// the same channel the caller uses to name a reduction back.
///
/// A path that cannot report the map at all reports NO names (empty, not
/// `None`-filled) — there is nothing to check there rather than nothing added,
/// so the guard stands down.
fn check_arjun_accounting(
    sub: &CnfFormula,
    ar: &vitri::bundle::PreprocessBundle,
    depth: u32,
    child_idx: usize,
) {
    let arjun_pow2 = ar.count_lift.arjun_pow2;
    let names = &ar.record.reduced_to_original_dimacs;
    let added = names.iter().filter(|name| name.is_none()).count() as u32;
    debug_assert!(
        names.is_empty()
            || ar.reduced.num_vars() + arjun_pow2 <= sub.num_vars() + added,
        "recovery Arjun over-count at depth {} child {}/2: \
         reduced {}v + free-mult 2^{} exceeds pre-Arjun {}v + {} added",
        depth,
        child_idx + 1,
        ar.reduced.num_vars(),
        arjun_pow2,
        sub.num_vars(),
        added,
    );
}

/// Max recovery depth (exclusive) at which each Shannon cofactor is re-Arjun'd
/// before its sub-compile. 1 = depth-0 only: only the first split's children get
/// projection cleanup.
const SPLIT_ARJUN_MAX_DEPTH: u32 = 1;

/// Log the recovery trigger and purge jemalloc pages.
///
/// Logs the recovery event to stderr, drops this thread's retained apply scratch
/// (`Engine::clear_scratch` — the sparse workspace and level pools the unwind
/// left pinned), then switches jemalloc to eager reclamation
/// ([`reclaim_eagerly`]) to advise the panicked parent's dirty pages away
/// before child compiles begin allocating.
///
/// It logs the [`HeapStats`] (`allocated` / `mapped` / `retained`) before and
/// after the purge, with the mallctl return code:
/// `allocated ≈ mapped` means live data the purge cannot reclaim, while
/// `allocated ≪ mapped + retained` means address space jemalloc holds, which
/// the purge gives back (under `retain:false` purged extents are unmapped).
fn log_and_purge_recovery(eng: &tididi::Engine, tag: &str, formula: &CnfFormula, depth: u32, what: &str) {
    const MIB: u64 = 1024 * 1024;
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
    let before = HeapStats::read();
    // Free this thread's retained apply scratch (chiefly the `RefCell`-owned
    // sparse workspace, which survives the panic-unwind of the failed compile
    // at full capacity) before the purge, so the purge can madvise the pages
    // the drop just freed.
    eng.clear_scratch();
    let reclaimed = reclaim_eagerly();
    let after = HeapStats::read();
    // The mallctl return code: "Ok" or the bare errno int.
    let rc = match reclaimed {
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
        (before.mapped + before.retained) / MIB,
        (after.mapped + after.retained) / MIB,
    );
}

/// Simplify `formula ∧ unit` via BCP closure, returning the simplified CNF
/// (or `None` if the conjunction is UNSAT).
///
/// Mechanism:
/// 1. Clone the original clauses and append `unit` as a unit clause.
/// 2. Ask Vitri's unit-propagation operation for the fixpoint —
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
