//! Python bindings for tididi-bonsai: a CNF goes in, as DIMACS text, as
//! clauses of DIMACS integers or as a file, and its count comes back with the
//! run's record.
//!
//! Each call is one `tididi_bonsai::count`: the parsing, the mode check, the
//! limits and the count are the library's, and this crate converts the
//! arguments and the answer.

use std::path::PathBuf;

use num_bigint::BigInt;
use num_rational::BigRational;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyFloat, PyString};
use tididi_bonsai::{Count, Input, Limits, Mode};

/// One class per class of `tididi_bonsai::Error`, whose documentation says
/// what each covers. The message is the library's own.
mod errors {
    use pyo3::create_exception;
    use pyo3::exceptions::PyException;
    use pyo3::prelude::*;

    create_exception!(tididi_bonsai, Error, PyException, "Base class of the errors tididi-bonsai raises.");
    create_exception!(
        tididi_bonsai,
        UsageError,
        Error,
        "The mode is unknown, contradicts the instance's header, or asks for an unweighted count of an instance with `c p weight` lines."
    );
    create_exception!(tididi_bonsai, EnvError, Error, "An environment setting the run reads is invalid.");
    create_exception!(tididi_bonsai, StageError, Error, "A preprocessing or compile stage failed.");
    create_exception!(
        tididi_bonsai,
        VtreeSpecError,
        Error,
        "The vtree construction cannot serve this run."
    );
    create_exception!(
        tididi_bonsai,
        InputError,
        Error,
        "The input could not be opened or read, or does not describe a formula."
    );
    create_exception!(
        tididi_bonsai,
        ArtifactError,
        Error,
        "The count could not be read off the compiled diagram."
    );
    create_exception!(
        tididi_bonsai,
        IncompleteError,
        Error,
        "The run gave up without a count, at the time limit or on a refused allocation."
    );

    /// The exception class for `error`'s class, carrying its message.
    /// Exhaustive, so a class added to the library gets an exception here
    /// before this crate builds again.
    pub(crate) fn of(error: tididi_bonsai::Error) -> PyErr {
        use tididi_bonsai::Error as E;
        let message = error.to_string();
        match error {
            E::Usage(_) => UsageError::new_err(message),
            E::Environment(_) => EnvError::new_err(message),
            E::Stage(_) => StageError::new_err(message),
            E::VtreeSpec(_) => VtreeSpecError::new_err(message),
            E::Input(_) => InputError::new_err(message),
            E::Artifact(_) => ArtifactError::new_err(message),
            E::Incomplete(_) => IncompleteError::new_err(message),
        }
    }

    pub(crate) fn add_to(module: &Bound<'_, PyModule>) -> PyResult<()> {
        let py = module.py();
        module.add("Error", py.get_type::<Error>())?;
        module.add("UsageError", py.get_type::<UsageError>())?;
        module.add("EnvError", py.get_type::<EnvError>())?;
        module.add("StageError", py.get_type::<StageError>())?;
        module.add("VtreeSpecError", py.get_type::<VtreeSpecError>())?;
        module.add("InputError", py.get_type::<InputError>())?;
        module.add("ArtifactError", py.get_type::<ArtifactError>())?;
        module.add("IncompleteError", py.get_type::<IncompleteError>())
    }
}

/// What a count answered: the mode the run resolved to, whether the formula
/// has a model, the count, and the run's record.
///
/// `count` and `stats` build new Python objects on each access, so keep what
/// they return rather than reading them in a loop.
#[pyclass(module = "tididi_bonsai", name = "Answer", frozen)]
pub struct Answer {
    mode: Mode,
    satisfiable: bool,
    count: Count,
    stats: String,
}

#[pymethods]
impl Answer {
    /// `"compile"`, `"mc"`, `"wmc"`, `"pmc"` or `"pwmc"`: the mode the run
    /// resolved to, from the instance's header and the mode it was given.
    #[getter]
    fn mode(&self) -> String {
        self.mode.to_string()
    }

    /// Whether the formula has a model. This is the run's verdict, not read
    /// off the count: a zero weight, or weights that cancel, make the weighted
    /// count of a satisfiable formula zero.
    #[getter]
    fn satisfiable(&self) -> bool {
        self.satisfiable
    }

    /// The count, over every variable the formula declares, or over the
    /// `c p show` variables on the projected modes: an exact `int` for
    /// `"compile"`, `"mc"` and `"pmc"`, an exact `fractions.Fraction` for
    /// `"pwmc"`, and a `float` for `"wmc"`, which counts in a bounded-precision
    /// log domain.
    #[getter]
    fn count<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        match &self.count {
            Count::Integer(count) => Ok(count.into_pyobject(py)?.into_any()),
            Count::Rational(count) => {
                py.import("fractions")?.getattr("Fraction")?.call1((count.numer(), count.denom()))
            }
            Count::Log(_) => Ok(self.count.to_f64().into_pyobject(py)?.into_any()),
        }
    }

    /// The run's record as a dict, the object `tididi --stats` writes: the
    /// mode, the count, sizes, timings and which stage answered. The user
    /// guide's `docs/stats.md` defines each field.
    #[getter]
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        py.import("json")?.call_method1("loads", (&self.stats,))
    }

    fn __repr__(&self) -> String {
        format!("<tididi_bonsai.Answer: {}, {}>", self.mode, self.count)
    }
}

impl Answer {
    fn of(answer: tididi_bonsai::Answer) -> Self {
        Answer {
            mode: answer.mode(),
            satisfiable: answer.satisfiable(),
            count: answer.count().clone(),
            stats: serde_json::to_string(answer.stats()).expect("the stats record serializes to JSON"),
        }
    }
}

/// The mode a keyword names, in the library's words when it names none.
fn mode(token: Option<&str>) -> PyResult<Option<Mode>> {
    token.map(str::parse::<Mode>).transpose().map_err(errors::of)
}

/// Count `input` under the mode and limits, with the interpreter lock
/// released.
fn run(
    py: Python<'_>,
    input: impl FnOnce() -> Result<Input, tididi_bonsai::Error> + Send,
    mode: Option<Mode>,
    limits: Limits,
) -> PyResult<Answer> {
    py.detach(|| tididi_bonsai::count(input()?, mode, &limits).map(Answer::of)).map_err(errors::of)
}

/// Count the models of a CNF, given as DIMACS text or as clauses, as
/// `tididi --mode MODE` counts the same formula in a file.
///
/// `formula` is one of:
///
/// - DIMACS text, as `str` or `bytes`, with its `c t`, `c p show` and
///   `c p weight` lines if it has them;
/// - clauses: an iterable of clauses, each an iterable of nonzero `int`
///   literals, `3` for variable 3 and `-3` for its negation, without the
///   closing `0`. A clause names each variable at most once;
/// - an object with a `clauses` attribute holding such clauses, and an `nv`
///   attribute with the variable count if it has one, as a PySAT `CNF` does.
///
/// Clauses take three more keywords, which DIMACS text declares in its own
/// lines instead:
///
/// - `num_vars`: the number of variables. The default is the object's `nv`,
///   or else the largest variable the clauses, `show` and `weights` name.
/// - `show`: the variables a projected count (`"pmc"`, `"pwmc"`) projects
///   onto, as an iterable of `int`; a `c p show` line.
/// - `weights`: literal weights for a weighted count (`"wmc"`, `"pwmc"`), a
///   mapping from a literal to its weight; `c p weight` lines. A weight is an
///   `int`, a `fractions.Fraction`, a decimal or fraction string such as
///   `"0.3"` or `"3/10"`, which is read exactly, or a `float`, which stands
///   for its shortest decimal: `0.1` weighs 1/10. A literal left out weighs 1.
///
/// Clauses carry no `c t` line, so give the mode: without one, a `show` set
/// makes the count projected and anything else compiles. The other keywords
/// are those of `count_file`.
///
/// Raises `InputError` for text that is not DIMACS, a literal or variable
/// outside the formula, a clause that names a variable twice, or a weight
/// string that is not a number; `UsageError` for a clause keyword given with
/// DIMACS text; and `TypeError` for a formula, clause or weight of another
/// type.
#[pyfunction]
#[pyo3(signature = (
    formula,
    *,
    mode = None,
    num_vars = None,
    show = None,
    weights = None,
    time_limit_ms = None,
    mem_limit_mb = 0
))]
#[allow(clippy::too_many_arguments)]
fn count(
    py: Python<'_>,
    formula: &Bound<'_, PyAny>,
    mode: Option<&str>,
    num_vars: Option<u32>,
    show: Option<&Bound<'_, PyAny>>,
    weights: Option<&Bound<'_, PyAny>>,
    time_limit_ms: Option<u64>,
    mem_limit_mb: u64,
) -> PyResult<Answer> {
    let mode = self::mode(mode)?;
    let limits = Limits { time_limit_ms, mem_limit_mb };
    let text;
    let bytes: &[u8] = if let Ok(bytes) = formula.cast::<PyBytes>() {
        bytes.as_bytes()
    } else if let Ok(string) = formula.cast::<PyString>() {
        text = string.to_str()?;
        text.as_bytes()
    } else {
        let formula = Clauses::read(formula, num_vars, show, weights)?;
        return run(py, move || formula.build(), mode, limits);
    };
    let given = [("num_vars", num_vars.is_some()), ("show", show.is_some()), ("weights", weights.is_some())];
    if let Some((keyword, _)) = given.into_iter().find(|&(_, given)| given) {
        return Err(errors::of(tididi_bonsai::Error::Usage(format!(
            "{keyword} applies to a formula given as clauses; DIMACS text declares its variables, \
             show set and weights in its own lines."
        ))));
    }
    run(py, || tididi_bonsai::parse_dimacs(bytes).map(Input::from), mode, limits)
}

/// A formula given as clauses, read out of its Python objects so the count
/// can run without the interpreter lock.
struct Clauses {
    num_vars: u32,
    clauses: Vec<Vec<i32>>,
    show: Option<Vec<u32>>,
    weights: Vec<(i32, BigRational)>,
}

impl Clauses {
    fn read(
        formula: &Bound<'_, PyAny>,
        num_vars: Option<u32>,
        show: Option<&Bound<'_, PyAny>>,
        weights: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let (clauses, nv) = if formula.hasattr("clauses")? {
            let nv = if formula.hasattr("nv")? { Some(formula.getattr("nv")?.extract()?) } else { None };
            (formula.getattr("clauses")?, nv)
        } else {
            (formula.clone(), None)
        };
        let clauses = clauses.try_iter().map_err(|_| {
            PyTypeError::new_err(format!(
                "formula expects DIMACS text (str or bytes), clauses (an iterable of iterables of int) \
                 or an object with a clauses attribute, got {}",
                formula.get_type().name().map_or_else(|_| "an unnamed type".into(), |name| name.to_string())
            ))
        })?;
        let clauses = clauses
            .map(|clause| clause?.try_iter()?.map(|literal| literal?.extract()).collect::<PyResult<Vec<i32>>>())
            .collect::<PyResult<Vec<Vec<i32>>>>()?;
        let show = show
            .map(|show| show.try_iter()?.map(|var| var?.extract()).collect::<PyResult<Vec<u32>>>())
            .transpose()?;
        let weights: Vec<(i32, BigRational)> = match weights {
            None => Vec::new(),
            Some(weights) if !weights.hasattr("items")? => {
                return Err(PyTypeError::new_err(format!(
                    "weights expects a mapping from literal to weight, got {}",
                    weights.get_type().name()?
                )));
            }
            Some(weights) => weights
                .call_method0("items")?
                .try_iter()?
                .map(|item| {
                    let (literal, value): (i32, Bound<'_, PyAny>) = item?.extract()?;
                    Ok((literal, weight(literal, &value)?))
                })
                .collect::<PyResult<_>>()?,
        };
        // The smallest variable space that holds every id given.
        let named = clauses.iter().flatten().chain(weights.iter().map(|(literal, _)| literal));
        let widest = named
            .map(|literal| literal.unsigned_abs())
            .chain(show.iter().flatten().copied())
            .max()
            .unwrap_or(0);
        Ok(Clauses { num_vars: num_vars.or(nv).unwrap_or(widest), clauses, show, weights })
    }

    fn build(self) -> Result<Input, tididi_bonsai::Error> {
        tididi_bonsai::formula_from_clauses(self.num_vars, &self.clauses, self.show.as_deref(), &self.weights)
            .map(Input::from)
    }
}

/// The weight `value` gives `literal`, exactly.
fn weight(literal: i32, value: &Bound<'_, PyAny>) -> PyResult<BigRational> {
    // A float stands for its shortest decimal, which Rust's formatting of an
    // `f64` writes without an exponent.
    let decimal;
    let text = if let Ok(float) = value.cast::<PyFloat>() {
        decimal = float.value().to_string();
        decimal.as_str()
    } else if let Ok(string) = value.cast::<PyString>() {
        string.to_str()?
    } else if value.hasattr("numerator")? && value.hasattr("denominator")? {
        let numerator: BigInt = value.getattr("numerator")?.extract()?;
        let denominator: BigInt = value.getattr("denominator")?.extract()?;
        if denominator == BigInt::ZERO {
            return Err(errors::of(tididi_bonsai::Error::Input(format!(
                "the weight of literal {literal} has denominator 0"
            ))));
        }
        return Ok(BigRational::new(numerator, denominator));
    } else {
        return Err(PyTypeError::new_err(format!(
            "the weight of literal {literal} is a {}; a weight is an int, a Fraction, a float or a string",
            value.get_type().name()?
        )));
    };
    // The reading `c p weight` lines get, so a string weighs what it would in
    // a DIMACS file.
    tididi_bonsai::vitri::cnf::parse_rational_weight(text).map_err(|error| {
        errors::of(tididi_bonsai::Error::Input(format!("the weight of literal {literal}: {error}")))
    })
}

/// Count the models of the DIMACS CNF file at `path`, as
/// `tididi --mode MODE PATH` does.
///
/// - `mode`: `"compile"`, `"mc"`, `"wmc"`, `"pmc"` or `"pwmc"`; `None` takes
///   the task the instance's `c t` and `c p show` lines declare, a plain
///   compile when they declare none. A mode given must agree with those
///   lines.
/// - `time_limit_ms`: the time the whole run may take, in milliseconds;
///   `None` sets no limit. The run checks it as it goes, so a step in
///   progress can finish after it, and a run that reaches it raises
///   `IncompleteError`.
/// - `mem_limit_mb`: lower the process's address-space limit (`RLIMIT_AS`) to
///   this many MiB when the run starts, so a model count nearing it splits the
///   formula instead of failing; `0` leaves it unchanged. The limit belongs to
///   the interpreter's process, and the call puts it back when it returns.
///
/// The user guide's `docs/limits.md` describes both limits.
///
/// Raises `InputError` for a file that cannot be read or is not DIMACS,
/// `UsageError` for a mode that is unknown or contradicts the header,
/// `IncompleteError` when the run gives up, and another `Error` subclass for
/// a run that fails. An unknown keyword or an argument of the wrong Python
/// type raises `TypeError`, a negative limit `OverflowError`.
///
/// The run prints its progress to the process's stderr, as the binary does.
/// The interpreter lock is released while it runs, so other Python threads
/// keep running; calls from several threads run one at a time.
#[pyfunction]
#[pyo3(signature = (path, *, mode = None, time_limit_ms = None, mem_limit_mb = 0))]
fn count_file(
    py: Python<'_>,
    path: PathBuf,
    mode: Option<&str>,
    time_limit_ms: Option<u64>,
    mem_limit_mb: u64,
) -> PyResult<Answer> {
    let mode = self::mode(mode)?;
    let limits = Limits { time_limit_ms, mem_limit_mb };
    run(py, || Ok(Input::File(path)), mode, limits)
}

// `pymodule` expands this function into a Rust module of the same name, so it
// cannot be called `tididi_bonsai` without shadowing the crate being wrapped.
// `name` is what Python imports.
/// Exact model counting by compiling a CNF into a Tree Decision Diagram: the
/// model count, the weighted count and their projections of a DIMACS formula.
#[pymodule]
#[pyo3(name = "tididi_bonsai")]
fn python_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    errors::add_to(module)?;
    module.add_class::<Answer>()?;
    module.add_function(wrap_pyfunction!(count, module)?)?;
    module.add_function(wrap_pyfunction!(count_file, module)?)?;
    // The `__init__.py` maturin generates around the extension re-exports
    // exactly this list.
    module.add(
        "__all__",
        vec![
            "Answer",
            "ArtifactError",
            "EnvError",
            "Error",
            "IncompleteError",
            "InputError",
            "StageError",
            "UsageError",
            "VtreeSpecError",
            "__version__",
            "count",
            "count_file",
        ],
    )
}
