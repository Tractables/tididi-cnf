//! tididi-cnf built for the browser: the C functions below wrap
//! [`tididi_cnf::count`], and draw the steps of a small formula's compile.
//!
//! Emscripten links a program, so `main` has to exist. It does nothing and the
//! page never runs it; the page calls the exported functions.
//!
//! Every string these functions return is NUL-terminated UTF-8, and the caller
//! releases it with [`tididi_cnf_string_free`].

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};
use tididi_cnf::compile::{ComponentPlan, CompileOptions, compile_cnf};
use tididi_cnf::tididi::io::write_tdd;
use tididi_cnf::tididi::vtree::VtreeNode;
use tididi_cnf::vitri::cnf::{Clause, CnfFormula, Original};
use tididi_cnf::vtree::{VtreeOptions, build_vtree};
use tididi_cnf::{Answer, Error, Limits, Mode};

/// The most variables a formula [`tididi_cnf_draw_json`] draws can have. The page
/// lays out every node of every step, and evaluates the diagrams in doubles,
/// which hold every count up to 2^53 exactly.
const DRAW_MAX_VARIABLES: u32 = 16;

fn main() {}

/// [`tididi_cnf::count`] on the `len` bytes of DIMACS at `dimacs`.
///
/// `mode` is null for the task the instance's header declares, or a mode's
/// token (`mc`, `wmc`, `pmc`, `pwmc`, `compile`), which the header must then
/// agree with. `time_limit_ms` is the run's time limit, or 0 for none. Both
/// cross the boundary as JavaScript values: a string and a number.
///
/// The answer is `{"ok": true, "mode", "satisfiable", "count", "value",
/// "stats"}`: the mode the run resolved to, its verdict, the count as the
/// binary prints it, the count's nearest `f64`
/// ([`Count::to_f64`](tididi_cnf::Count::to_f64), `null` past an `f64`'s
/// range), and the record the binary's `--stats` writes. A run without an
/// answer is `{"ok": false, "error": {"kind", "message"}}`, `kind` being
/// [`Error::kind`](tididi_cnf::Error::kind), or `panic` for a panic that
/// reached this function.
///
/// # Safety
///
/// `dimacs` points to `len` readable bytes, or is null or `len` is 0, which is
/// the empty input. `mode` is null or points to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_count_json(
    dimacs: *const u8,
    len: usize,
    mode: *const c_char,
    time_limit_ms: u32,
) -> *mut c_char {
    // SAFETY: the caller's guarantee for `dimacs` is the one `input` needs.
    let dimacs = unsafe { input(dimacs, len) };
    let mode = (!mode.is_null()).then(|| {
        // SAFETY: the caller guarantees a NUL-terminated string.
        String::from_utf8_lossy(unsafe { CStr::from_ptr(mode) }.to_bytes()).into_owned()
    });
    let limits = Limits {
        time_limit_ms: (time_limit_ms > 0).then_some(u64::from(time_limit_ms)),
        ..Limits::default()
    };
    reply(|| count(dimacs, mode.as_deref(), &limits))
}

/// The steps of compiling the `len` bytes of DIMACS at `dimacs`, for the page
/// to draw: the formula as it stands, without the preprocessing
/// [`tididi_cnf_count_json`] runs, on the vtree vitri's portfolio builds for it.
///
/// The compile works bottom-up over the vtree: each clause waits at the lowest
/// node whose subtree holds its variables, and each internal node conjoins
/// its children's diagrams and then its own clauses. A step is one internal
/// node and the diagram of every clause under it, which is the same diagram
/// whichever way it was built: the library's diagrams are canonical.
///
/// The answer is `{"ok": true, "clauses", "weights", "vtree", "steps"}`:
/// the clauses as DIMACS integers; each variable's `[negative, positive]`
/// literal weights, as exact rationals in text, or null when the formula
/// declares none; the vtree in the `.vtree` text format; and one
/// `{"node", "tdd"}` per internal vtree node, its id in `vtree` and the
/// diagram in the library's `.tdd` text format ([`write_tdd`]). A formula
/// with fewer than 2 or more than [`DRAW_MAX_VARIABLES`] variables is
/// `{"ok": false, "error": {"kind": "size", "message"}}`, and other errors are
/// as [`tididi_cnf_count_json`] reports them.
///
/// # Safety
///
/// As for [`tididi_cnf_count_json`]'s `dimacs` and `len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_draw_json(dimacs: *const u8, len: usize) -> *mut c_char {
    // SAFETY: the caller's guarantee for `dimacs` is the one `input` needs.
    let dimacs = unsafe { input(dimacs, len) };
    reply(|| draw(dimacs))
}

/// Release a string [`tididi_cnf_count_json`] or [`tididi_cnf_draw_json`] returned.
///
/// # Safety
///
/// `text` is null or a string one of them returned that nothing has released
/// since.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_string_free(text: *mut c_char) {
    if !text.is_null() {
        // SAFETY: the pointer came from `CString::into_raw` in `reply`, and
        // this is its only release point.
        drop(unsafe { CString::from_raw(text) });
    }
}

/// The `len` bytes at `bytes`: the empty input when `bytes` is null or `len`
/// is 0.
///
/// # Safety
///
/// `bytes` is null, or points to `len` readable bytes that outlive the call
/// this input is for.
unsafe fn input<'a>(bytes: *const u8, len: usize) -> &'a [u8] {
    if bytes.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `len` readable bytes at `bytes`.
        unsafe { std::slice::from_raw_parts(bytes, len) }
    }
}

/// `answer()` as a string the caller releases with [`tididi_cnf_string_free`].
fn reply(answer: impl FnOnce() -> Value) -> *mut c_char {
    // A panic reaching Emscripten aborts the module, and every later call on
    // it fails; the panic hook has already written the details to stderr.
    let answer = catch_unwind(AssertUnwindSafe(answer))
        .unwrap_or_else(|_| failure("panic", "tididi-cnf panicked; please report it as a bug"));
    // JSON escapes a NUL inside a string, so serialized JSON never holds one.
    CString::new(answer.to_string())
        .expect("serialized JSON contains no NUL byte")
        .into_raw()
}

/// The answer as JSON: the count and the record, or the error.
fn count(dimacs: &[u8], mode: Option<&str>, limits: &Limits) -> Value {
    let answer = mode.map(str::parse::<Mode>).transpose().and_then(|mode| {
        tididi_cnf::count(tididi_cnf::parse_dimacs(dimacs)?, mode, limits)
    });
    match answer {
        Ok(answer) => answered(&answer),
        Err(error) => failure(error.kind(), &error.to_string()),
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

/// The drawing as JSON, or the error.
fn draw(dimacs: &[u8]) -> Value {
    let (formula, meta) = match tididi_cnf::parse_dimacs(dimacs) {
        Ok(parsed) => parsed,
        Err(error) => return failure(error.kind(), &error.to_string()),
    };
    let variables = formula.num_vars();
    if !(2..=DRAW_MAX_VARIABLES).contains(&variables) {
        let message = format!(
            "the page draws formulas of 2 to {DRAW_MAX_VARIABLES} variables; this one has {variables}"
        );
        return failure("size", &message);
    }
    let clauses: Vec<Vec<i32>> = formula
        .clauses()
        .iter()
        .map(|clause| clause.literals.iter().map(|literal| literal.to_dimacs()).collect())
        .collect();
    let weights = meta.declared_weights().map(|table| {
        let weights = table.resolve::<Original>(variables as usize);
        let text = weights.as_pairs().iter().map(|(negative, positive)| {
            [negative.to_string(), positive.to_string()]
        });
        text.collect::<Vec<_>>()
    });
    match steps(&formula) {
        Ok((vtree, steps)) => json!({
            "ok": true, "clauses": clauses, "weights": weights, "vtree": vtree, "steps": steps,
        }),
        Err(error) => failure(error.kind(), &error.to_string()),
    }
}

/// The vtree, as text, and each internal node's step.
fn steps(formula: &CnfFormula) -> Result<(String, Vec<Value>), Error> {
    let vtree = build_vtree(formula, "portfolio", &VtreeOptions::default())?;
    // The variables under each vtree node, one bit each.
    let mut under = vec![0u32; vtree.num_nodes()];
    for t in vtree.bottomup() {
        under[t.idx()] = match vtree.node(t) {
            VtreeNode::Leaf { var, .. } => 1 << var.idx(),
            VtreeNode::Internal { left, right, .. } => under[left.idx()] | under[right.idx()],
        };
    }
    let variables = |clause: &Clause| {
        clause.literals.iter().fold(0u32, |mask, literal| mask | 1 << literal.var.idx())
    };
    let failed = |error: &dyn std::fmt::Display| Error::Artifact(format!("the drawing: {error}"));
    let steps = vtree.context().run(|engine| {
        let step = |t: usize| {
            let clauses = formula.clauses().iter().filter(|c| variables(c) & !under[t] == 0);
            let part = CnfFormula::new(formula.num_vars(), clauses.cloned().collect())
                .map_err(|error| failed(&error))?;
            // The diagram on `vtree` itself, not on the trees a split into
            // components would build.
            let whole = CompileOptions { plan: ComponentPlan::Whole, ..CompileOptions::default() };
            let diagram = compile_cnf(engine, &part, &vtree, whole)?.diagram;
            let mut tdd = Vec::new();
            write_tdd(&mut tdd, &diagram).map_err(|error| failed(&error))?;
            String::from_utf8(tdd).map_err(|error| failed(&error))
        };
        vtree
            .internal_bottomup()
            .map(|(t, _, _)| Ok(json!({ "node": vtree.topo_pos(t), "tdd": step(t.idx())? })))
            .collect::<Result<Vec<_>, Error>>()
    })?;
    Ok((vtree.to_text(), steps))
}
