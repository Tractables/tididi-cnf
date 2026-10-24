//! One call per task: [`count`], what it reads, and what it answers.
//!
//! [`count`] is the run the `tididi` binary makes, minus the printing: the
//! binary parses its command line into the same mode and [`Limits`], calls the
//! same `driver::solve`, and prints the [`Answer`] it gets back.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use num_bigint::BigUint;
use num_rational::BigRational;
use tididi::diagram::SignedLog;
use tididi::Tdd;
use vitri::cnf::{CnfFormula, CnfMeta};

use crate::config::{Limits, Mode, Settings};
use crate::output::Stats;
use crate::Error;

/// The instance a run reads: a DIMACS file, or a formula already parsed with
/// its header.
///
/// Every form [`count`] takes converts into this, so a path (`&str`, `String`,
/// `&Path`, `PathBuf`) or the pair [`parse_dimacs`](crate::parse_dimacs) and
/// [`formula_from_clauses`](crate::formula_from_clauses) return can be passed
/// as it is.
#[derive(Debug)]
pub enum Input {
    /// A DIMACS file, read when the run starts. Its reading counts against
    /// the time limit, as it does in the binary.
    ///
    /// A directory is read as the bundle `vitri --out-dir` writes. The run
    /// compiles the bundle's `reduced.cnf` on its `vtree.vtree`, split into
    /// the components `components.json` lists, and preprocesses nothing; its
    /// mode is the one in `preprocess.json`, and a different `mode` is
    /// refused. The answer is the original formula's: the record's lift is
    /// applied to the count, and in mode compile the diagram is expanded back
    /// onto the original variables.
    File(PathBuf),
    /// A formula and the header lines that came with it: the `c t` mode, the
    /// `c p show` set and the `c p weight` table.
    Formula(CnfFormula, CnfMeta),
}

impl From<&str> for Input {
    fn from(path: &str) -> Self {
        Input::File(path.into())
    }
}

impl From<String> for Input {
    fn from(path: String) -> Self {
        Input::File(path.into())
    }
}

impl From<&std::path::Path> for Input {
    fn from(path: &std::path::Path) -> Self {
        Input::File(path.into())
    }
}

impl From<PathBuf> for Input {
    fn from(path: PathBuf) -> Self {
        Input::File(path)
    }
}

impl From<(CnfFormula, CnfMeta)> for Input {
    fn from((formula, meta): (CnfFormula, CnfMeta)) -> Self {
        Input::Formula(formula, meta)
    }
}

/// A count, in the domain its task produces.
///
/// It prints as the binary's answer line prints it: an integer in decimal, a
/// rational as `<numerator>/<denominator>`, and a log-domain value as
/// `<mantissa>e<exponent>` (`0` for zero).
#[derive(Clone, Debug, PartialEq)]
pub enum Count {
    /// An exact integer: the model count ([`Mode::Mc`], [`Mode::Compile`]) and
    /// the projected model count ([`Mode::Pmc`]).
    Integer(BigUint),
    /// An exact rational: the projected weighted count ([`Mode::Pwmc`]).
    Rational(BigRational),
    /// A value in the bounded-precision log domain, its sign and the natural
    /// log of its magnitude: the weighted count ([`Mode::Wmc`]), which stays in
    /// that domain because exact rationals grow without bound on large
    /// instances.
    Log(SignedLog),
}

impl Count {
    /// The count as the nearest `f64`, for printing or plotting.
    ///
    /// Lossy: a large integer or rational is rounded to an `f64`'s precision,
    /// and a magnitude beyond `f64`'s range, as a log-domain value's can be,
    /// is infinite.
    ///
    /// ```
    /// let count = tididi_cnf::Count::Rational(num_rational::BigRational::new(91.into(), 125.into()));
    /// assert_eq!(count.to_f64(), 0.728);
    /// ```
    pub fn to_f64(&self) -> f64 {
        use num_traits::ToPrimitive;
        match self {
            Count::Integer(count) => count.to_f64().unwrap_or(f64::INFINITY),
            // A rational with a nonzero denominator always converts.
            Count::Rational(count) => count.to_f64().unwrap_or(f64::NAN),
            // Zero is sign 0 with a magnitude of exp(-inf) = 0.
            Count::Log(count) => f64::from(count.sign) * count.ln_abs.exp(),
        }
    }
}

impl std::fmt::Display for Count {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Count::Integer(count) => write!(f, "{count}"),
            // Always both parts, so a whole number still reads as a rational.
            Count::Rational(count) => write!(f, "{}/{}", count.numer(), count.denom()),
            Count::Log(count) => {
                if count.sign == 0 {
                    return f.write_str("0");
                }
                // Decompose 10^log10 into mantissa·10^exp with mantissa in
                // [1, 10): the magnitude can be far outside what an f64 holds.
                let log10 = count.log10_abs();
                let exp = log10.floor();
                let mantissa = 10f64.powf(log10 - exp);
                let sign = if count.sign < 0 { "-" } else { "" };
                write!(f, "{sign}{mantissa}e{}", exp as i64)
            }
        }
    }
}

/// What a run answered: the mode it ran, whether the formula has a model, the
/// count, and the run's record.
#[derive(Debug)]
pub struct Answer {
    pub(crate) mode: Mode,
    pub(crate) satisfiable: bool,
    pub(crate) count: Count,
    pub(crate) diagram: Option<Tdd>,
    pub(crate) stats: Stats,
}

impl Answer {
    /// The mode the run resolved to, from the header and the mode it was
    /// given.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether the formula has a model. This is the run's verdict, not read
    /// off the count: a zero weight, or weights that cancel, make the weighted
    /// count of a satisfiable formula zero.
    pub fn satisfiable(&self) -> bool {
        self.satisfiable
    }

    /// The count: over every variable the formula declares, or over the
    /// `c p show` variables on the projected modes.
    pub fn count(&self) -> &Count {
        &self.count
    }

    /// The compiled diagram, over every variable the formula declares, in the
    /// [`Mode::Compile`] mode; `None` in the counting modes, which count
    /// without keeping one. [`tididi::io::save_tdd`] writes it, as the
    /// binary's `--save-tdd` does.
    pub fn diagram(&self) -> Option<&Tdd> {
        self.diagram.as_ref()
    }

    /// The run's record: sizes, timings, which stage answered, as the binary's
    /// `--stats` writes it.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
}

/// A run that stopped without an answer: why, and the run's record when it
/// got far enough to open one — what it was doing when it stopped, which is
/// the row worth reading. The record is boxed so a failure stays as small as
/// the error.
pub(crate) struct Failure {
    pub(crate) error: Error,
    pub(crate) stats: Option<Box<Stats>>,
}

/// Count, or compile, one instance: the task `mode` names, or the one its
/// header declares, under `limits`.
///
/// `input` is a DIMACS file path or a parsed [`Input::Formula`]. The mode is
/// the header's when `mode` is `None` — a `c t mc|wmc|pmc|pwmc` line, made
/// projected by a `c p show` line — and a plain compile when the header
/// declares none; a `mode` given must agree with the header. The run is the
/// binary's without its printing: the same preprocessing, vtree selection and
/// fallbacks, the same answer, and the same record in [`Answer::stats`]. Its
/// progress lines go to stderr, as the binary's do. Calls from several
/// threads run one at a time: a call waits for the one in progress to return.
///
/// # Errors
///
/// - [`Error::Input`]: the file cannot be opened, or is not DIMACS; or a
///   supplied formula's metadata names a show or weight variable outside
///   that formula's declared variable space.
/// - [`Error::Usage`]: `mode` contradicts the header, or asks for an
///   unweighted count ([`Mode::Mc`] or [`Mode::Pmc`]) of an instance with
///   `c p weight` lines, which that count would ignore.
/// - [`Error::Environment`]: a vitri setting in the environment is invalid.
///   Each call reads the environment as it is when the call starts.
/// - [`Error::Stage`], [`Error::VtreeSpec`]: preprocessing or vtree
///   construction failed.
/// - [`Error::Incomplete`]: the run ended without a count: at the time limit,
///   on a refused allocation (reading the count off the diagram included), or
///   when every stage gave up.
///
/// # Example
///
/// ```
/// use tididi_cnf::{Count, Limits, Mode};
///
/// // (x1 ∨ x2) ∧ (¬x1 ∨ x3) ∧ (¬x2 ∨ ¬x3), with no header: a model count
/// // needs the mode.
/// let answer = tididi_cnf::count("tests/data/simple.cnf", Some(Mode::Mc), &Limits::default())?;
/// assert_eq!(answer.count(), &Count::Integer(2u32.into()));
/// # Ok::<(), tididi_cnf::Error>(())
/// ```
pub fn count(input: impl Into<Input>, mode: Option<Mode>, limits: &Limits) -> Result<Answer, Error> {
    let _one_at_a_time = ONE_RUN.lock().unwrap_or_else(PoisonError::into_inner);
    crate::driver::solve(input.into(), mode, limits, &Settings::default())
        .map_err(|failure| failure.error)
}

/// Held for the whole of every [`count`]: the memory limit and the memory
/// readings belong to the process, and preprocessing goes through vitri, which
/// is not known to be safe to enter from two threads at once.
static ONE_RUN: Mutex<()> = Mutex::new(());
