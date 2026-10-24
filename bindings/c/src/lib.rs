//! The C API for tididi-cnf: a CNF goes in, as DIMACS text, as clauses of
//! DIMACS integers or as a file, and its count comes back with the run's
//! record.
//!
//! Each call is one `tididi_cnf::count`: the parsing, the mode check, the
//! limits and the count are the library's, and this crate converts the
//! arguments and the answer. `include/tididi_cnf.h` is generated from this
//! file by cbindgen, so the documentation comments on the exported items are
//! what a C caller reads. `README.md` beside this crate covers building and
//! linking.
//!
//! Every exported function that runs code able to panic runs it under
//! `catch_unwind`, so no unwind reaches the caller.

#![allow(non_camel_case_types)]

use std::any::Any;
use std::ffi::{CStr, CString, OsStr, c_char};
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::{ptr, slice};

use tididi_cnf::{Answer, Error, Input, Limits, Mode};

/// The version of the ABI this header declares. `tididi_cnf_abi_version`
/// returns the version of the library that is actually loaded. It is raised
/// whenever a signature, a status code or an ownership rule changes in a way
/// that breaks a program built against the previous header.
pub const TIDIDI_CNF_ABI_VERSION: u32 = 1;

/// A status code: `TIDIDI_CNF_OK`, or one of the `TIDIDI_CNF_ERROR_`
/// values.
pub type tididi_cnf_code = i32;

/// The call succeeded.
pub const TIDIDI_CNF_OK: tididi_cnf_code = 0;
/// The mode is unknown, contradicts the instance's header, or asks for an
/// unweighted count of an instance with `c p weight` lines.
pub const TIDIDI_CNF_ERROR_USAGE: tididi_cnf_code = 1;
/// An environment setting the run reads is invalid.
pub const TIDIDI_CNF_ERROR_ENVIRONMENT: tididi_cnf_code = 2;
/// A preprocessing or compile stage failed.
pub const TIDIDI_CNF_ERROR_STAGE: tididi_cnf_code = 3;
/// The vtree construction cannot serve this run.
pub const TIDIDI_CNF_ERROR_VTREE_SPEC: tididi_cnf_code = 4;
/// The input could not be opened or read, or does not describe a formula.
pub const TIDIDI_CNF_ERROR_INPUT: tididi_cnf_code = 5;
/// The count could not be read off the compiled diagram.
pub const TIDIDI_CNF_ERROR_ARTIFACT: tididi_cnf_code = 6;
/// The run ended without a count: at the time limit, on a refused allocation,
/// or when every stage gave up.
pub const TIDIDI_CNF_ERROR_INCOMPLETE: tididi_cnf_code = 7;
/// An argument broke this header's contract, such as a null pointer with a
/// nonzero length.
pub const TIDIDI_CNF_ERROR_INVALID_ARGUMENT: tididi_cnf_code = 8;
/// tididi-cnf panicked. The panic stopped at this boundary and its message
/// is the error message; report it as a bug.
pub const TIDIDI_CNF_ERROR_PANIC: tididi_cnf_code = 9;

/// What one count call produced: the answer, or the error that stopped the
/// run.
///
/// Opaque. Read it with the `tididi_cnf_answer_` functions and release it
/// with `tididi_cnf_answer_free`. An answer does not change after the call
/// returns it, so several threads may read one at the same time; free it
/// once, after the last read.
pub struct tididi_cnf_answer {
    code: tididi_cnf_code,
    outcome: Outcome,
}

enum Outcome {
    Counted(Counted),
    Failed(Text),
}

struct Counted {
    mode: Text,
    satisfiable: bool,
    count: Text,
    count_f64: f64,
    stats: Text,
}

/// Bytes lent to C, stored with one NUL after them that their length does not
/// count.
struct Text(Box<[u8]>);

impl Text {
    fn new(bytes: impl Into<Vec<u8>>) -> Self {
        let mut bytes = bytes.into();
        bytes.push(0);
        Text(bytes.into_boxed_slice())
    }

    fn len(&self) -> usize {
        self.0.len() - 1
    }
}

impl tididi_cnf_answer {
    fn counted(answer: &Answer) -> Self {
        let stats = serde_json::to_string(answer.stats()).expect("the stats record serializes to JSON");
        tididi_cnf_answer {
            code: TIDIDI_CNF_OK,
            outcome: Outcome::Counted(Counted {
                mode: Text::new(answer.mode().to_string()),
                satisfiable: answer.satisfiable(),
                count: Text::new(answer.count().to_string()),
                count_f64: answer.count().to_f64(),
                stats: Text::new(stats),
            }),
        }
    }

    fn failed(code: tididi_cnf_code, message: String) -> Self {
        tididi_cnf_answer { code, outcome: Outcome::Failed(Text::new(message)) }
    }

    fn counts(&self) -> Option<&Counted> {
        match &self.outcome {
            Outcome::Counted(counted) => Some(counted),
            Outcome::Failed(_) => None,
        }
    }

    fn message(&self) -> Option<&Text> {
        match &self.outcome {
            Outcome::Counted(_) => None,
            Outcome::Failed(message) => Some(message),
        }
    }
}

/// What stopped a count call.
enum Refusal {
    /// An argument broke the header's contract; the message says which.
    Argument(String),
    /// tididi-cnf refused the input or the mode, or the run failed.
    Compiler(Error),
}

impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        Refusal::Compiler(error)
    }
}

/// The ABI version of the loaded library, which is the
/// `TIDIDI_CNF_ABI_VERSION` it was built with.
#[unsafe(no_mangle)]
pub extern "C" fn tididi_cnf_abi_version() -> u32 {
    TIDIDI_CNF_ABI_VERSION
}

/// The version of tididi-cnf inside the library, such as `0.1.0`, as a
/// NUL-terminated string. The string is static: the caller does not free it,
/// and it stays valid for as long as the library is loaded.
#[unsafe(no_mangle)]
pub extern "C" fn tididi_cnf_version() -> *const c_char {
    static VERSION: OnceLock<CString> = OnceLock::new();
    catch_unwind(|| {
        VERSION
            .get_or_init(|| CString::new(env!("CARGO_PKG_VERSION")).unwrap_or_default())
            .as_ptr()
    })
    .unwrap_or(c"".as_ptr())
}

/// Count the models of a DIMACS CNF given as text, as `tididi --mode MODE`
/// counts the same text in a file.
///
/// `dimacs` points to `dimacs_len` bytes of DIMACS text, which need not end in
/// a NUL, with its `c t`, `c p show` and `c p weight` lines if it has them; it
/// may be null when `dimacs_len` is 0. The other arguments and the result are
/// those of `tididi_cnf_count_file`.
///
/// # Safety
///
/// `dimacs` must be null or point to `dimacs_len` readable bytes that do not
/// change during the call; `mode` and `out` as for `tididi_cnf_count_file`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_count(
    dimacs: *const u8,
    dimacs_len: usize,
    mode: *const c_char,
    time_limit_ms: u64,
    mem_limit_mb: u64,
    out: *mut *mut tididi_cnf_answer,
) -> tididi_cnf_code {
    unsafe {
        deliver(out, || {
            let dimacs = borrow(dimacs, dimacs_len, "dimacs")?;
            let input = Input::from(tididi_cnf::parse_dimacs(dimacs)?);
            count(input, mode, time_limit_ms, mem_limit_mb)
        })
    }
}

/// One literal's weight, for `tididi_cnf_count_clauses`: a `c p weight`
/// line.
#[repr(C)]
pub struct tididi_cnf_weight {
    /// The literal: `3` for variable 3, `-3` for its negation.
    pub literal: i32,
    /// The weight as NUL-terminated text, read as a `c p weight` line's is,
    /// exactly: a decimal such as `0.3` or `-2.5e-3`, a fraction such as
    /// `3/10`, or an integer.
    pub weight: *const c_char,
}

/// Count the models of a CNF given as clauses of DIMACS integers, as `tididi
/// --mode MODE` counts the same formula written in a file.
///
/// The formula has `num_vars` variables. `literals` points to `literals_len`
/// literals, the clauses one after another, each closed by a `0` as in the
/// clause lines of a DIMACS file: `3` is variable 3 and `-3` its negation. The
/// array ends with the `0` that closes its last clause, and a clause names each
/// variable at most once. `literals` may be null when `literals_len` is 0, a
/// formula with no clauses.
///
/// `show` points to `show_len` variables, the `c p show` set a projected count
/// (`pmc`, `pwmc`) projects onto; null declares no show set, while a non-null
/// `show` with `show_len` 0 declares the empty one. `weights` points to
/// `weights_len` literal weights for a weighted count (`wmc`, `pwmc`), the
/// `c p weight` lines, read as the competition's format reads them: a literal
/// they leave out weighs `1 - w` when the other literal of its variable is given
/// a `w` strictly between 0 and 1, and 1 when neither is given; a literal given
/// twice takes its last weight, and `weights` may be null when `weights_len` is
/// 0.
/// Clauses carry no `c t` line, so pass the mode: with a null `mode`, a show
/// set makes the count projected and anything else compiles.
///
/// The other arguments and the result are those of
/// `tididi_cnf_count_file`. A literal, shown variable or weight literal
/// that names no variable of the formula (0, `INT32_MIN`, or a variable above
/// `num_vars`), a clause that names a variable twice, a weight that is not a
/// number, and a literal given alone at a weight outside (0, 1), which leaves
/// its partner none, return `TIDIDI_CNF_ERROR_INPUT`; `literals` not ending
/// with 0 returns `TIDIDI_CNF_ERROR_INVALID_ARGUMENT`.
///
/// # Safety
///
/// `literals`, `show` and `weights` must each be null or point to as many
/// readable elements as its length says, each weight's `weight` must be null
/// or a NUL-terminated string, none of them may change during the call, and
/// `mode` and `out` are as for `tididi_cnf_count_file`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn tididi_cnf_count_clauses(
    num_vars: u32,
    literals: *const i32,
    literals_len: usize,
    show: *const u32,
    show_len: usize,
    weights: *const tididi_cnf_weight,
    weights_len: usize,
    mode: *const c_char,
    time_limit_ms: u64,
    mem_limit_mb: u64,
    out: *mut *mut tididi_cnf_answer,
) -> tididi_cnf_code {
    unsafe {
        deliver(out, || {
            let literals = borrow(literals, literals_len, "literals")?;
            let clauses = match literals.split_last() {
                None => None,
                Some((0, closed)) => Some(closed.split(|&literal| literal == 0)),
                Some((last, _)) => {
                    return Err(Refusal::Argument(format!(
                        "literals ends with {last}, not with the 0 that closes its last clause; \
                         is literals_len {literals_len} right?"
                    )));
                }
            };
            let declared = borrow(show, show_len, "show")?;
            let show = (!show.is_null()).then_some(declared);
            let weights = borrow(weights, weights_len, "weights")?
                .iter()
                .enumerate()
                .map(|(i, weight)| {
                    let text = c_string(weight.weight)
                        .ok_or_else(|| Refusal::Argument(format!("weights[{i}].weight is null")))?
                        .to_str()
                        .map_err(|e| Refusal::Argument(format!("weights[{i}].weight is not UTF-8 text: {e}")))?;
                    // The reading `c p weight` lines get, so a weight is what
                    // it would be in a DIMACS file.
                    let value = tididi_cnf::vitri::cnf::parse_rational_weight(text).map_err(|error| {
                        Error::Input(format!("the weight of literal {}: {error}", weight.literal))
                    })?;
                    Ok((weight.literal, value))
                })
                .collect::<Result<Vec<_>, Refusal>>()?;
            let formula = tididi_cnf::formula_from_clauses(num_vars, clauses.into_iter().flatten(), show, &weights)?;
            count(Input::from(formula), mode, time_limit_ms, mem_limit_mb)
        })
    }
}

/// Count the models of the DIMACS CNF file at `path`, or of the vitri bundle
/// in the directory at `path` (the user guide's `docs/input.md`), as
/// `tididi --mode MODE PATH` does.
///
/// `path` is a NUL-terminated file name. `mode` is a NUL-terminated mode
/// token, one of `compile`, `mc`, `wmc`, `pmc` and `pwmc`, or null to take
/// the task the instance's `c t` and `c p show` lines declare, a plain
/// compile when they declare none; a mode given must agree with those lines,
/// or with the mode a bundle was made for.
/// `time_limit_ms` limits the whole run, and 0 sets no limit. `mem_limit_mb`
/// lowers the process's address-space limit (`RLIMIT_AS`) to that many MiB
/// while the run lasts, and 0 leaves it unchanged; the limit belongs to the
/// process, and the call puts it back when it returns. The library's `Limits`
/// documents both, and the user guide's `docs/limits.md` what a run that
/// reaches one does: the time limit is checked as the run goes, so a step in
/// progress can finish after it.
///
/// Returns `TIDIDI_CNF_OK` or a `TIDIDI_CNF_ERROR_` code. Unless `out`
/// is null, `*out` receives a new answer whatever the code, and the code
/// equals `tididi_cnf_answer_code(*out)`: the count on success, the error
/// message otherwise. The caller owns `*out` and releases it with
/// `tididi_cnf_answer_free`. A null `out` returns
/// `TIDIDI_CNF_ERROR_INVALID_ARGUMENT` and produces no answer. Nothing
/// keeps a reference to the arguments after the call returns.
///
/// The run prints its progress to stderr, as the binary does. Calls run one
/// at a time: a call made while another thread's call is running waits for it
/// to finish.
///
/// # Safety
///
/// `path` must be null or a NUL-terminated string, `mode` must be null or a
/// NUL-terminated string, neither may change during the call, and `out` must
/// be null or point to storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_count_file(
    path: *const c_char,
    mode: *const c_char,
    time_limit_ms: u64,
    mem_limit_mb: u64,
    out: *mut *mut tididi_cnf_answer,
) -> tididi_cnf_code {
    unsafe {
        deliver(out, || {
            let path = c_string(path).ok_or_else(|| Refusal::Argument("path is null".to_string()))?;
            let input = Input::File(PathBuf::from(OsStr::from_bytes(path.to_bytes())));
            count(input, mode, time_limit_ms, mem_limit_mb)
        })
    }
}

/// Release an answer. Null is ignored.
///
/// # Safety
///
/// `answer` must be null or an answer a count call produced that has not been
/// freed. Every buffer borrowed from it becomes invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_free(answer: *mut tididi_cnf_answer) {
    if !answer.is_null() {
        drop(unsafe { Box::from_raw(answer) });
    }
}

/// The code the count call returned with this answer: `TIDIDI_CNF_OK` or a
/// `TIDIDI_CNF_ERROR_` value. `TIDIDI_CNF_ERROR_INVALID_ARGUMENT` for a
/// null `answer`.
///
/// # Safety
///
/// `answer` must be null or a live answer from a count call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_code(
    answer: *const tididi_cnf_answer,
) -> tididi_cnf_code {
    unsafe { answer.as_ref() }.map_or(TIDIDI_CNF_ERROR_INVALID_ARGUMENT, |answer| answer.code)
}

/// The mode the run resolved to, from the instance's header and the mode it
/// was given: `compile`, `mc`, `wmc`, `pmc` or `pwmc`.
///
/// Writes the token's length to `*len` when `len` is not null. The bytes
/// belong to `answer`: they stay valid and unchanged until
/// `tididi_cnf_answer_free(answer)`, and are followed by a NUL that `*len`
/// does not count. Null, with `*len` set to 0, for a failed or null `answer`.
///
/// # Safety
///
/// `answer` must be null or a live answer from a count call, and `len` must be
/// null or point to storage for one `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_mode(
    answer: *const tididi_cnf_answer,
    len: *mut usize,
) -> *const c_char {
    let counted = unsafe { answer.as_ref() }.and_then(tididi_cnf_answer::counts);
    unsafe { lend(counted.map(|counted| &counted.mode), len) }
}

/// Whether the formula has a model. This is the run's verdict, not read off
/// the count: a zero weight, or weights that cancel, make the weighted count
/// of a satisfiable formula zero. False for a failed or null `answer`.
///
/// # Safety
///
/// `answer` must be null or a live answer from a count call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_satisfiable(answer: *const tididi_cnf_answer) -> bool {
    let counted = unsafe { answer.as_ref() }.and_then(tididi_cnf_answer::counts);
    counted.is_some_and(|counted| counted.satisfiable)
}

/// The count, spelled as the binary's answer line spells it: a decimal
/// integer for `compile`, `mc` and `pmc`; an exact fraction in lowest terms,
/// `<numerator>/<denominator>`, for `pwmc`; and for `wmc`, which counts in a
/// bounded-precision log domain, `<mantissa>e<exponent>` with the mantissa in
/// [1, 10) and signed when negative, or `0`. The count is over every variable
/// the formula declares, or over the `c p show` variables on the projected
/// modes.
///
/// Writes the text's length to `*len` when `len` is not null. The bytes belong
/// to `answer`: they stay valid and unchanged until
/// `tididi_cnf_answer_free(answer)`, and are followed by a NUL that `*len`
/// does not count. Null, with `*len` set to 0, for a failed or null `answer`.
///
/// # Safety
///
/// `answer` must be null or a live answer from a count call, and `len` must be
/// null or point to storage for one `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_count(
    answer: *const tididi_cnf_answer,
    len: *mut usize,
) -> *const c_char {
    let counted = unsafe { answer.as_ref() }.and_then(tididi_cnf_answer::counts);
    unsafe { lend(counted.map(|counted| &counted.count), len) }
}

/// The count as the nearest `double`, for printing or plotting: lossy, since
/// a large count rounds to a `double`'s precision and a magnitude past its
/// range is infinite. `tididi_cnf_answer_count` spells the exact count.
/// NaN for a failed or null `answer`.
///
/// # Safety
///
/// `answer` must be null or a live answer from a count call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_count_f64(answer: *const tididi_cnf_answer) -> f64 {
    let counted = unsafe { answer.as_ref() }.and_then(tididi_cnf_answer::counts);
    counted.map_or(f64::NAN, |counted| counted.count_f64)
}

/// The run's record as one JSON object, the one `tididi --stats` writes:
/// the mode, the count, sizes, timings and which stage answered. The user
/// guide's `docs/stats.md` defines each field.
///
/// Writes the text's length to `*len` when `len` is not null. The bytes belong
/// to `answer`: they stay valid and unchanged until
/// `tididi_cnf_answer_free(answer)`, and are followed by a NUL that `*len`
/// does not count. Null, with `*len` set to 0, for a failed or null `answer`.
///
/// # Safety
///
/// `answer` must be null or a live answer from a count call, and `len` must be
/// null or point to storage for one `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_stats_json(
    answer: *const tididi_cnf_answer,
    len: *mut usize,
) -> *const c_char {
    let counted = unsafe { answer.as_ref() }.and_then(tididi_cnf_answer::counts);
    unsafe { lend(counted.map(|counted| &counted.stats), len) }
}

/// The error message: UTF-8 text saying what went wrong, as the binary prints
/// it on stderr. It names the input line, mode or setting at fault where there
/// is one. A run that gave up (`TIDIDI_CNF_ERROR_INCOMPLETE`) says what
/// ended it; what stopped each stage went to stderr as the run went.
///
/// Writes the message's length to `*len` when `len` is not null. The bytes
/// belong to `answer`: they stay valid and unchanged until
/// `tididi_cnf_answer_free(answer)`, and are followed by a NUL that `*len`
/// does not count. Null, with `*len` set to 0, for a successful or null
/// `answer`.
///
/// # Safety
///
/// `answer` must be null or a live answer from a count call, and `len` must be
/// null or point to storage for one `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tididi_cnf_answer_error_message(
    answer: *const tididi_cnf_answer,
    len: *mut usize,
) -> *const c_char {
    let message = unsafe { answer.as_ref() }.and_then(tididi_cnf_answer::message);
    unsafe { lend(message, len) }
}

/// Run `body`, turning a panic into an answer that carries its message, and
/// hand the answer to `*out`.
///
/// # Safety
///
/// `out` must be null or point to storage for one pointer.
unsafe fn deliver(
    out: *mut *mut tididi_cnf_answer,
    body: impl FnOnce() -> Result<Answer, Refusal>,
) -> tididi_cnf_code {
    if out.is_null() {
        return TIDIDI_CNF_ERROR_INVALID_ARGUMENT;
    }
    let result = answer(|| match body() {
        Ok(counted) => tididi_cnf_answer::counted(&counted),
        Err(Refusal::Argument(message)) => tididi_cnf_answer::failed(TIDIDI_CNF_ERROR_INVALID_ARGUMENT, message),
        Err(Refusal::Compiler(error)) => tididi_cnf_answer::failed(code_of(&error), error.to_string()),
    });
    let code = result.code;
    unsafe { out.write(Box::into_raw(Box::new(result))) };
    code
}

/// Run `body`, turning a panic into an answer that carries its message.
fn answer(body: impl FnOnce() -> tididi_cnf_answer) -> tididi_cnf_answer {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|payload| {
        tididi_cnf_answer::failed(TIDIDI_CNF_ERROR_PANIC, panic_message(&*payload))
    })
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    let what = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    match what {
        Some(what) => format!("tididi-cnf panicked: {what}"),
        None => "tididi-cnf panicked".to_string(),
    }
}

/// The status code for an error the library returned. Exhaustive, so a class
/// added to the library gets a code here before this crate builds again.
fn code_of(error: &Error) -> tididi_cnf_code {
    match error {
        Error::Usage(_) => TIDIDI_CNF_ERROR_USAGE,
        Error::Environment(_) => TIDIDI_CNF_ERROR_ENVIRONMENT,
        Error::Stage(_) => TIDIDI_CNF_ERROR_STAGE,
        Error::VtreeSpec(_) => TIDIDI_CNF_ERROR_VTREE_SPEC,
        Error::Input(_) => TIDIDI_CNF_ERROR_INPUT,
        Error::Artifact(_) => TIDIDI_CNF_ERROR_ARTIFACT,
        Error::Incomplete(_) => TIDIDI_CNF_ERROR_INCOMPLETE,
    }
}

/// Read the mode and the limits, and count `input` under them.
///
/// # Safety
///
/// `mode` must be null or a NUL-terminated string.
unsafe fn count(
    input: Input,
    mode: *const c_char,
    time_limit_ms: u64,
    mem_limit_mb: u64,
) -> Result<Answer, Refusal> {
    let mode = match unsafe { c_string(mode) } {
        None => None,
        Some(token) => {
            let token = token
                .to_str()
                .map_err(|e| Refusal::Argument(format!("mode is not UTF-8 text: {e}")))?;
            Some(token.parse::<Mode>()?)
        }
    };
    let limits = Limits { time_limit_ms: (time_limit_ms != 0).then_some(time_limit_ms), mem_limit_mb };
    Ok(tididi_cnf::count(input, mode, &limits)?)
}

/// The NUL-terminated string at `text`, or `None` for null.
///
/// # Safety
///
/// `text` must be null or a NUL-terminated string that does not change while
/// the borrow is in use.
unsafe fn c_string<'a>(text: *const c_char) -> Option<&'a CStr> {
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) })
}

/// The `len` elements at `data`, empty for a null `data` with `len` 0. `name`
/// is the argument's name in the header, for the refusal.
///
/// # Safety
///
/// `data` must be null or point to `len` readable elements that do not change
/// while the slice is in use.
unsafe fn borrow<'a, T>(data: *const T, len: usize, name: &str) -> Result<&'a [T], Refusal> {
    if data.is_null() {
        return match len {
            0 => Ok(&[]),
            _ => Err(Refusal::Argument(format!("{name} is null but {name}_len is {len}"))),
        };
    }
    if len.checked_mul(size_of::<T>()).is_none_or(|bytes| isize::try_from(bytes).is_err()) {
        return Err(Refusal::Argument(format!("{name}_len is {len}, larger than any buffer")));
    }
    Ok(unsafe { slice::from_raw_parts(data, len) })
}

/// Point C at `text`, and write its length to `len` when `len` is not null.
/// Null and 0 when there is no text.
///
/// # Safety
///
/// `len` must be null or point to storage for one `size_t`.
unsafe fn lend(text: Option<&Text>, len: *mut usize) -> *const c_char {
    let (at, n) = text.map_or((ptr::null(), 0), |text| (text.0.as_ptr().cast(), text.len()));
    if !len.is_null() {
        unsafe { len.write(n) };
    }
    at
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
