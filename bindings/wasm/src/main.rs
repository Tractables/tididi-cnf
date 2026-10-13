//! tididi-bonsai built for the browser: the C functions below wrap
//! [`tididi_bonsai::count`].
//!
//! Emscripten links a program, so `main` has to exist. It does nothing and the
//! page never runs it; the page calls the exported functions.
//!
//! Every string these functions return is NUL-terminated UTF-8, and the caller
//! releases it with [`bonsai_string_free`].

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};
use tididi_bonsai::{Answer, Error, Limits, Mode};

fn main() {}

/// [`tididi_bonsai::count`] on the `len` bytes of DIMACS at `dimacs`.
///
/// `mode` is null for the task the instance's header declares, or a mode's
/// token (`mc`, `wmc`, `pmc`, `pwmc`, `compile`), which the header must then
/// agree with. `time_limit_ms` is the run's time limit, or 0 for none. Both
/// cross the boundary as JavaScript values: a string and a number.
///
/// The answer is `{"ok": true, "mode", "satisfiable", "count", "value",
/// "stats"}`: the mode the run resolved to, its verdict, the count as the
/// binary prints it, the count's nearest `f64` ([`Count::to_f64`](tididi_bonsai::Count::to_f64), `null`
/// past an `f64`'s range), and the record the binary's `--stats` writes.
/// A run without an answer
/// is `{"ok": false, "error": {"kind", "message"}}`, `kind` naming the
/// [`Error`] variant as the record's `exit_reason` does, or `panic` for a
/// panic that reached this function.
///
/// # Safety
///
/// `dimacs` points to `len` readable bytes, or is null or `len` is 0, which is
/// the empty input. `mode` is null or points to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bonsai_count_json(
    dimacs: *const u8,
    len: usize,
    mode: *const c_char,
    time_limit_ms: u32,
) -> *mut c_char {
    let dimacs = if dimacs.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: the caller guarantees `len` readable bytes at `dimacs`.
        unsafe { std::slice::from_raw_parts(dimacs, len) }
    };
    let mode = (!mode.is_null()).then(|| {
        // SAFETY: the caller guarantees a NUL-terminated string.
        String::from_utf8_lossy(unsafe { CStr::from_ptr(mode) }.to_bytes()).into_owned()
    });
    let limits = Limits {
        time_limit_ms: (time_limit_ms > 0).then_some(u64::from(time_limit_ms)),
        ..Limits::default()
    };
    // A panic reaching Emscripten aborts the module, and every later call on
    // it fails; the panic hook has already written the details to stderr.
    let answer = catch_unwind(AssertUnwindSafe(|| count(dimacs, mode.as_deref(), &limits)))
        .unwrap_or_else(|_| failure("panic", "tididi-bonsai panicked; please report it as a bug"));
    // JSON escapes a NUL inside a string, so serialized JSON never holds one.
    CString::new(answer.to_string())
        .expect("serialized JSON contains no NUL byte")
        .into_raw()
}

/// Release a string [`bonsai_count_json`] returned.
///
/// # Safety
///
/// `text` is null or a string [`bonsai_count_json`] returned that nothing has
/// released since.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bonsai_string_free(text: *mut c_char) {
    if !text.is_null() {
        // SAFETY: the pointer came from `CString::into_raw` in
        // `bonsai_count_json`, and this is its only release point.
        drop(unsafe { CString::from_raw(text) });
    }
}

/// The answer as JSON: the count and the record, or the error.
fn count(dimacs: &[u8], mode: Option<&str>, limits: &Limits) -> Value {
    let answer = mode.map(str::parse::<Mode>).transpose().and_then(|mode| {
        tididi_bonsai::count(tididi_bonsai::parse_dimacs(dimacs)?, mode, limits)
    });
    match answer {
        Ok(answer) => answered(&answer),
        Err(error) => failure(kind(&error), &error.to_string()),
    }
}

fn answered(answer: &Answer) -> Value {
    json!({
        "ok": true,
        "mode": answer.mode().to_string(),
        "satisfiable": answer.satisfiable(),
        "count": answer.count().to_string(),
        // JSON has no infinity: a non-finite value serializes as null.
        "value": answer.count().to_f64(),
        "stats": answer.stats(),
    })
}

fn failure(kind: &str, message: &str) -> Value {
    json!({ "ok": false, "error": { "kind": kind, "message": message } })
}

/// The error's class, as the record's `exit_reason` names it.
fn kind(error: &Error) -> &'static str {
    match error {
        Error::Usage(_) => "usage",
        Error::Environment(_) => "environment",
        Error::Stage(_) => "stage",
        Error::VtreeSpec(_) => "vtree-spec",
        Error::Input(_) => "input",
        Error::Artifact(_) => "artifact",
        Error::Incomplete(_) => "incomplete",
    }
}
