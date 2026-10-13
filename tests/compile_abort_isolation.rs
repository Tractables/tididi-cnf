//! Compiles run back to back on one thread (the recovery cascade's split
//! children, an embedder looping over instances) share the engine's limits, the
//! apply pipeline's retained scratch and the allocator's accounting. A compile
//! cut short by its deadline must leave that state as it found it, or the next
//! compile dies at once against a limit it never reached. Checked three ways:
//! the cut leaves no apply deadline armed, the retained scratch gives back the
//! aborted compile's high-water capacity, and the same formula compiled again
//! counts what a first compile counts.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::make_formula;
use tididi_bonsai::vtree::VtreeOptions;
use vitri::cnf::CnfFormula;
use tididi_bonsai::compile::{CompileOptions, compile_cnf};
use tididi::Engine;
use tididi::diagram::Tdd;
use tididi::limits::StopRules;
use tididi::vtree::Vtree;

/// Process-wide live allocated bytes, jemalloc's `stats.allocated`, after an
/// epoch advance refreshes the counters (they lag raw alloc/free events until
/// it does). The crate installs jemalloc as the global allocator, so this reads
/// the heap the compiles below allocate from.
fn heap_bytes() -> u64 {
    let _ = tikv_jemalloc_ctl::epoch::advance();
    tikv_jemalloc_ctl::stats::allocated::read().unwrap_or(0) as u64
}

/// A formula that compiles quickly from a clean slate, though far slower than
/// [`CUT_AFTER`].
fn subject() -> (CnfFormula, Arc<Vtree>) {
    let (num_vars, clauses) = common::queens_cnf(8);
    let f = make_formula(num_vars, clauses);
    let built = tididi_bonsai::vtree::build_vtree_with_components(
        &f,
        "portfolio",
        &VtreeOptions::default(),
    )
    .unwrap();
    assert!(
        built.components.is_none(),
        "this test seeds ONE vtree, so the subject formula must be connected",
    );
    (f, built.root)
}

/// How far out the abort tests set their deadline. The subject takes far
/// longer than this to compile, so the compile is cut; where the cut lands,
/// between steps or inside an apply, varies from run to run, and every property
/// checked here holds wherever it lands.
const CUT_AFTER: Duration = Duration::from_millis(2);

/// Compile under a deadline [`CUT_AFTER`] from now. Returns the diagram, `None`
/// on a give-up.
fn compile_cut(eng: &Engine, f: &CnfFormula, v: &Arc<Vtree>) -> Option<Tdd> {
    let deadline = Some(Instant::now() + CUT_AFTER);
    compile_cnf(eng, f, v, CompileOptions { deadline, ..CompileOptions::default() }).ok().map(|compiled| compiled.diagram)
}

/// One 8-queens subject, one engine, three properties an aborted compile must
/// leave true for whatever runs after it on the same thread.
#[test]
fn a_deadline_abort_leaves_the_engine_reusable() {
    let eng = &Engine::new();
    let (f, v) = subject();

    // Reference first, from a clean process state, before any abort: what a
    // first-and-only compile of this formula counts.
    let reference = compile_cnf(eng, &f, &v, CompileOptions::default())
        .expect("the subject formula compiles without a deadline")
        .diagram
        .model_count()
        .unwrap();

    // The deadline a compile arms as its apply-level cut lives exactly as long
    // as the compile. An aborted compile must not leave it armed on the
    // engine: it has passed by then, so the next compile, which sets no
    // deadline of its own and keeps whatever is armed, would stop at its
    // first apply. `compile_cnf` scopes it, and a cut returns early, so this
    // is the path whose leak matters.
    assert!(compile_cut(eng, &f, &v).is_none(), "a deadline this close must cut the compile");
    assert_eq!(
        eng.limits().armed().stop_rules(),
        StopRules::default(),
        "the aborted compile left its apply deadline armed on the engine; the next compile \
         would stop at its first apply",
    );

    // The apply pipeline's thread-local scratch pools are grow-only by
    // design, so a compile that ballooned leaves them at ITS high-water
    // capacity — live bytes, charged to whoever measures the heap next. A
    // driver that sizes a memory ceiling relative to the live heap therefore
    // reads a different baseline for every arm (measured: 1.3 GiB → 11.8 GiB
    // across the arms of one run), which is enough on its own to make
    // sequential arms non-comparable.
    //
    // `Engine::clear_scratch` is the one drain for that state. This pins that
    // it actually recovers an aborted compile's retained capacity, so a
    // caller that calls it between compiles gets a stable baseline.
    let before = heap_bytes();
    assert!(compile_cut(eng, &f, &v).is_none(), "a deadline this close must cut the compile");
    eng.clear_scratch();
    let after = heap_bytes();
    // Generous slack: the point is that the aborted compile's retained pools are
    // GONE, not that the allocator is byte-identical (the formula, the vtree and
    // the harness's own bookkeeping are live in both readings).
    assert!(
        after <= before + (16 << 20),
        "after clear_scratch the live heap is still {} MiB above the pre-compile \
         baseline ({} MiB) — the aborted compile's retained scratch was not released, so any \
         ceiling a caller derives from the live heap is charged for it",
        (after.saturating_sub(before)) >> 20,
        before >> 20,
    );

    // The behavioral statement, and the one a harness actually depends on: a
    // compile that follows an aborted one must behave like a first-and-only
    // compile. Same formula, same vtree, no limit of its own — it has to
    // finish, and its count has to be the count.
    //
    // Deliberately does NOT call `Engine::clear_scratch` between this abort
    // and the recompile: the claim under test is that the LIBRARY leaves the
    // thread usable, not that a caller can clean up after it. (A caller still
    // should — see above — but for the baseline it measures, not for
    // correctness.)
    assert!(compile_cut(eng, &f, &v).is_none(), "a deadline this close must cut the compile");
    let again = compile_cnf(eng, &f, &v, CompileOptions::default()).unwrap_or_else(|_| {
        panic!(
            "the compile AFTER a deadline abort gave up, though the same compile finishes \
             from a clean process — the abort poisoned the thread",
        )
    })
    .diagram;
    assert_eq!(
        again.model_count().unwrap(),
        reference,
        "the post-abort compile finished but counted a different number than a \
         first-and-only compile of the same formula",
    );
}
