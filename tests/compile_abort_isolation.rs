//! Compiles run back to back on one thread (the recovery cascade's split
//! children, an embedder looping over instances) share the engine's limits, the
//! apply pipeline's retained scratch and the allocator's accounting. A compile
//! cut short must leave that state as it found it, or the next compile dies at
//! once against a limit it never reached. Checked three ways: the cut leaves
//! the compile's own deadline unarmed, the retained scratch gives back the
//! aborted compile's high-water capacity, and the same formula compiled again
//! counts what a first compile counts.
//!
//! It is one test, alone in its binary, because the heap reading is
//! process-wide.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use vitri::cnf::CnfFormula;

use tididi::Engine;
use tididi::limits::StopRules;
use tididi::vtree::Vtree;
use tididi_bonsai::compile::{CompileOptions, compile_cnf};
use tididi_bonsai::vtree::VtreeOptions;

use common::make_formula;

/// Process-wide live allocated bytes, jemalloc's `stats.allocated`, after an
/// epoch advance refreshes the counters (they lag raw alloc/free events until
/// it does). The crate installs jemalloc as the global allocator, so this reads
/// the heap the compiles below allocate from.
fn heap_bytes() -> u64 {
    let _ = tikv_jemalloc_ctl::epoch::advance();
    tikv_jemalloc_ctl::stats::allocated::read().unwrap_or(0) as u64
}

/// Eight queens on one connected vtree: a formula that compiles quickly from a
/// clean slate.
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
        "this test seeds one vtree, so the subject formula must be connected",
    );
    (f, built.root)
}

/// A compile with a deadline of its own, as a caller running it under a time
/// limit sets one, far enough out that the compile never reaches it.
fn with_deadline() -> CompileOptions<'static> {
    CompileOptions { deadline: Some(Instant::now() + Duration::from_secs(3600)), ..CompileOptions::default() }
}

/// One 8-queens subject, one engine, three properties an aborted compile must
/// leave true for whatever runs after it on the same thread. Each abort is a
/// cut halfway through the work a whole compile of the subject takes, so it
/// lands at the same point, inside an apply or between two, on every machine.
#[test]
fn an_aborted_compile_leaves_the_engine_reusable() {
    let eng = &Engine::new();
    let (f, v) = subject();

    // Reference first, from a clean process state, before any abort: what a
    // first-and-only compile of this formula counts, and the work it takes.
    let start = eng.limits().work_units();
    let reference = compile_cnf(eng, &f, &v, with_deadline())
        .expect("the subject formula compiles before its deadline")
        .diagram
        .model_count()
        .unwrap();
    let half = (eng.limits().work_units() - start) / 2;
    assert!(half > 0, "the subject's compile does no work the cut could stop");

    // The deadline a compile arms as its apply-level cut lives exactly as long
    // as the compile. An aborted compile must not leave it armed on the
    // engine: the next compile, which keeps whatever is armed, would stop
    // when it passed. `compile_cnf` scopes it, and a cut returns early, so
    // this is the path whose leak matters. The check is made inside the cut,
    // whose own scope would otherwise restore the engine's settings.
    let compile_cut = || {
        common::cut_after(eng, half, || {
            let compiled = compile_cnf(eng, &f, &v, with_deadline());
            assert_eq!(
                eng.limits().armed().stop_rules(),
                StopRules::default(),
                "the aborted compile left its apply deadline armed on the engine",
            );
            compiled.is_err()
        })
    };
    assert!(compile_cut(), "a cut halfway through the work must stop the compile");

    // The apply pipeline's thread-local scratch pools are grow-only by
    // design, so a compile that ballooned leaves them at its own high-water
    // capacity: live bytes, charged to whoever measures the heap next. A
    // caller that sizes a memory ceiling relative to the live heap would
    // read a different baseline after every compile.
    //
    // `Engine::clear_scratch` is the one drain for that state. This pins that
    // it actually recovers an aborted compile's retained capacity, so a
    // caller that calls it between compiles gets a stable baseline.
    let before = heap_bytes();
    assert!(compile_cut(), "a cut halfway through the work must stop the compile");
    eng.clear_scratch();
    let after = heap_bytes();
    // Generous slack: the point is that the aborted compile's retained pools
    // are released, not that the allocator is byte-identical (the formula,
    // the vtree and the test's own bookkeeping are live in both readings).
    assert!(
        after <= before + (16 << 20),
        "after clear_scratch the live heap is still {} MiB above the pre-compile \
         baseline ({} MiB) — the aborted compile's retained scratch was not released, so any \
         ceiling a caller derives from the live heap is charged for it",
        (after.saturating_sub(before)) >> 20,
        before >> 20,
    );

    // The behavioural statement, and the one a caller depends on: a compile
    // that follows an aborted one behaves like a first-and-only compile.
    // Same formula, same vtree, no limit of its own: it has to finish, and
    // its count has to be the count.
    //
    // No `Engine::clear_scratch` runs between this abort and the recompile:
    // the claim under test is that the library leaves the thread usable, not
    // that a caller can clean up after it. A caller still should, for the
    // baseline it measures, though not for correctness.
    assert!(compile_cut(), "a cut halfway through the work must stop the compile");
    let again = compile_cnf(eng, &f, &v, CompileOptions::default()).unwrap_or_else(|_| {
        panic!(
            "the compile after an aborted one gave up, though the same compile finishes \
             from a clean process: the abort poisoned the thread",
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
