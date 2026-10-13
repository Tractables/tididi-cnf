//! What a run is asked for, and the configuration record it runs on.
//!
//! [`Mode`] and [`Limits`] are what a caller of [`crate::count`] chooses;
//! `Settings` is the rest of what the command line can set, and
//! `Settings::refuse_inert` refuses whichever of it the run's mode would
//! ignore. `Config` is the record every stage below the driver reads, built
//! from the limits, the settings and vitri's environment settings by
//! `Config::new`; [`crate::vtree::VtreeOptions`] is the per-request half of it
//! that travels with one vtree construction. Nothing here reads argv, the
//! environment or a file: `cli` fills `Limits` and `Settings` from the command
//! line, `frontend::request` reads vitri's environment settings, and the
//! stages read the resulting `Config`.

/// The `--preprocess` level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreprocessLevel {
    /// Pass the input formula through without simplification.
    None,
    /// Run vitri's full simplification pipeline.
    Full,
}

impl PreprocessLevel {
    /// Every level, in the order `--help` lists them.
    pub(crate) const ALL: [PreprocessLevel; 2] = [PreprocessLevel::None, PreprocessLevel::Full];

    /// The level's name, as `--preprocess` spells it.
    pub(crate) fn token(self) -> &'static str {
        match self {
            PreprocessLevel::None => "none",
            PreprocessLevel::Full => "full",
        }
    }
}

/// The limits a run works under: the time it may take and the address space
/// it may use.
///
/// These are the binary's `--time-limit-ms` and `--mem-limit-mb`, with the
/// same meaning. `Limits::default()` sets neither.
///
/// ```
/// let limits = tididi_bonsai::Limits { time_limit_ms: Some(60_000), ..Default::default() };
/// assert_eq!(limits.mem_limit_mb, 0);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// The time the whole run may take, in milliseconds, counted from the
    /// moment it starts. Every stage budget is a share of it, and a run that
    /// reaches it stops with [`Error::Incomplete`](crate::Error::Incomplete).
    /// `None` sets no limit, and the stages use fixed budgets of their own.
    pub time_limit_ms: Option<u64>,
    /// Lower the process's address-space limit (`RLIMIT_AS`) to this many MiB
    /// while the run lasts, as `ulimit -v` does, so the stages that adapt to
    /// memory pressure can see the limit: a compile that runs out of memory
    /// splits the formula instead of failing. The run only ever lowers the
    /// limit, never raises it, and puts back the one it found when it
    /// returns. `0` leaves it unchanged, the library's default. The binary's
    /// `--mem-limit-mb` has a default of its own, which `tididi --help`
    /// prints. A WebAssembly build has no such limit and refuses any other
    /// value with [`Error::Usage`](crate::Error::Usage).
    pub mem_limit_mb: u64,
}

/// What the command line sets beyond the mode and the limits: `--vtree`,
/// `--preprocess`, whether `--save-tdd` asks for the circuit, and the
/// `TIDIDI_SAT_PRUNE` switch. The library's [`crate::count`] leaves all four
/// unset, so none of them can be inert there.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settings {
    /// `--vtree`: compile on this spec instead of the default construction.
    pub(crate) vtree: Option<String>,
    /// `--preprocess`, when it was on the command line.
    pub(crate) preprocess: Option<PreprocessLevel>,
    /// Whether `--save-tdd` was on the command line. The run must then be a
    /// plain compile, whose circuit the command line writes out.
    pub(crate) save_tdd: bool,
    /// `TIDIDI_SAT_PRUNE`, when it is set: `Some(false)` for `off`,
    /// `Some(true)` for `on`. The weighted count refuses it set either way,
    /// since its compiles never prune.
    pub(crate) sat_prune: Option<bool>,
}

impl Settings {
    /// Refuse a setting the run's `mode` would ignore. Every such check is
    /// here, and the driver makes it once, as soon as the instance's header
    /// has decided the mode and before any preprocessing. Each check is on
    /// the setting's presence, and its message names the setting and what
    /// made the run that mode: the `--mode` it was given (`flag`) or the
    /// header lines of `meta`.
    pub(crate) fn refuse_inert(
        &self,
        flag: Option<Mode>,
        meta: &vitri::cnf::CnfMeta,
        mode: Mode,
    ) -> Result<(), crate::Error> {
        let cause = || match (flag, Mode::declared(meta)) {
            (Some(flag), _) => format!("--mode {flag}"),
            (None, Some((_, lines))) => lines,
            (None, None) => "the default".to_owned(),
        };
        // Only a compile builds the circuit over every original variable
        // whose count is the one reported.
        if self.save_tdd && mode != Mode::Compile {
            return Err(crate::Error::Usage(format!(
                "--save-tdd needs mode compile, but {} asks for a count (this run's \
                 mode is {mode}). A counting run compiles a preprocessed formula whose diagram \
                 does not carry the reported count, so there is no faithful circuit to save. \
                 Drop --save-tdd to compute the count.",
                cause()
            )));
        }
        // `--preprocess` sets the simplify stage of the frontend the compile,
        // MC and WMC share. The projected counts preprocess through the
        // projection chain instead, which always simplifies.
        if self.preprocess.is_some() && mode.projected() {
            return Err(crate::Error::Usage(format!(
                "--preprocess applies to the compile, mc and wmc modes, but {} makes \
                 this run a projected count (mode {mode}), whose projection chain always simplifies. \
                 Drop --preprocess.",
                cause()
            )));
        }
        // The weighted count has one construction, its cascade on vitri's
        // portfolio.
        if self.vtree.is_some() && mode == Mode::Wmc {
            return Err(crate::Error::Usage(format!(
                "--vtree applies to the compile, mc, pmc and pwmc modes, but {} makes \
                 this run a weighted count (mode wmc), which always compiles through its cascade \
                 on portfolio. Drop --vtree.",
                cause()
            )));
        }
        // The weighted count compiles with a weight store, which the SAT-prune
        // pass cannot read, so its compiles never prune. The projected
        // weighted count compiles without one and prunes.
        if self.sat_prune.is_some() && mode == Mode::Wmc {
            return Err(crate::Error::Environment(format!(
                "TIDIDI_SAT_PRUNE is set, but it applies to the compile, mc, pmc and pwmc \
                 modes, and {} makes this run a weighted count (mode wmc), whose \
                 compiles never run SAT pruning. Unset TIDIDI_SAT_PRUNE.",
                cause()
            )));
        }
        Ok(())
    }
}

/// The run's settings: what the caller asked for, plus `mode`,
/// `source_structure_profile` and `no_sbva_retry_armed`, which the driver fills
/// from the instance, and what the run's vtree constructions share,
/// `portfolio_history` and `vtree_time`.
pub(crate) struct Config {
    /// `--vtree`, when the caller gave it. Without it the model and weighted
    /// counts run their cascades; with it the model count compiles on it
    /// directly and the weighted count refuses it.
    pub(crate) vtree: Option<String>,
    /// The portfolio's record of the vtrees it has built, shared by every
    /// vtree request of the run.
    pub(crate) portfolio_history: vitri::decompose::PortfolioBuildHistory,
    /// The time the run's vtree constructions take, which every stage that
    /// builds one adds to and the record reports as `vtree_ms`.
    pub(crate) vtree_time: crate::budget::VtreeTime,
    /// vitri's structure profile of the source formula before preprocessing,
    /// measured once by the driver and copied into every vtree-selection
    /// request of the run.
    pub(crate) source_structure_profile: Option<vitri::score::StructureProfile>,
    /// Whether the parent cascade should retry a coloring-like input with
    /// vitri's SBVA policy forced off after the primary compile fails.
    pub(crate) no_sbva_retry_armed: bool,
    /// Whether the frontend simplifies the formula before the compile.
    pub(crate) preprocess_level: PreprocessLevel,
    /// What the run answers, as `driver::parse_input` resolves it from the
    /// mode the caller named and the instance header through
    /// [`Mode::resolve`]. Read, never re-derived, below the driver.
    pub(crate) mode: Mode,
    /// The run's time limit in ms, [`Limits::time_limit_ms`]. Internal budgets
    /// (Arjun, the Shannon split, SAT pruning) are shares of it; without it the
    /// absolute defaults apply.
    pub(crate) time_limit_ms: Option<u64>,
    /// Whether the run's compiles may run the SAT-prune pass: on unless
    /// `TIDIDI_SAT_PRUNE=off`, a diagnostic off-switch for paired on/off
    /// measurements. The driver hands the run's compiles a SAT-prune pass
    /// ([`crate::compile::sat_prune::SatPrune`]) only when it is set.
    pub(crate) sat_prune: bool,
    /// vitri's `VITRI_*` settings, read and checked from the environment as
    /// the run starts: the base of every vitri request the run makes.
    pub(crate) vitri: vitri::RunConfig,
}

impl Config {
    /// The record a run starts from: `limits`, `settings` and vitri's
    /// settings as the caller gave them, the preprocessing defaulting to
    /// [`PreprocessLevel::Full`]. The
    /// mode is resolved once the instance is read, so it holds
    /// [`Mode::Compile`] until then.
    pub(crate) fn new(limits: &Limits, settings: &Settings, vitri: vitri::RunConfig) -> Config {
        Config {
            vtree: settings.vtree.clone(),
            portfolio_history: vitri::decompose::PortfolioBuildHistory::default(),
            vtree_time: crate::budget::VtreeTime::default(),
            source_structure_profile: None,
            no_sbva_retry_armed: false,
            preprocess_level: settings.preprocess.unwrap_or(PreprocessLevel::Full),
            mode: Mode::Compile,
            time_limit_ms: limits.time_limit_ms,
            sat_prune: settings.sat_prune.unwrap_or(true),
            vitri,
        }
    }

    /// The vtree the run compiles on: `--vtree`, or `portfolio` without it.
    pub(crate) fn vtree_spec(&self) -> &str {
        self.vtree.as_deref().unwrap_or("portfolio")
    }
}

/// What a run answers: one of the four counting tasks, or a compile.
///
/// The instance header declares it: a `c t mc|wmc|pmc|pwmc` line names the
/// count, and a `c p show` line makes it projected. A mode given explicitly —
/// the binary's `--mode`, or the `mode` argument of [`crate::count`] — must
/// agree with that header, and without either the run is a
/// [`Compile`](Mode::Compile). The two are merged once, when the instance is
/// read; every dispatch reads the result and nothing re-derives it.
///
/// It prints as the token the header, `--mode` and the stats record spell it
/// with: `compile`, `mc`, `wmc`, `pmc` or `pwmc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Compile the formula into a diagram over every original variable, and
    /// count its models.
    Compile,
    /// Count the models: exact, an integer.
    Mc,
    /// Weigh the models by the `c p weight` table: in a bounded-precision log
    /// domain.
    Wmc,
    /// Count the assignments to the `c p show` variables that extend to a
    /// model: exact, an integer.
    Pmc,
    /// Weigh those assignments by the `c p weight` table: exact, a rational.
    Pwmc,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.token())
    }
}

/// The mode a token names, spelled as it prints, so a caller that takes the
/// mode as text (the C and Python bindings) reads it with the binary's
/// vocabulary.
///
/// ```
/// use tididi_bonsai::Mode;
///
/// assert_eq!("pwmc".parse::<Mode>()?, Mode::Pwmc);
/// assert!("weighted".parse::<Mode>().is_err());
/// # Ok::<(), tididi_bonsai::Error>(())
/// ```
impl std::str::FromStr for Mode {
    type Err = crate::Error;

    /// [`Error::Usage`](crate::Error::Usage), listing the tokens, for any
    /// other spelling.
    fn from_str(token: &str) -> Result<Self, crate::Error> {
        one_of("mode", token, &Mode::ALL, Mode::token).map_err(crate::Error::Usage)
    }
}

impl Mode {
    /// Every mode, in the order `--help` lists them.
    pub(crate) const ALL: [Mode; 5] = [Mode::Compile, Mode::Mc, Mode::Wmc, Mode::Pmc, Mode::Pwmc];

    /// The vitri mode of the same name, whose token is this mode's.
    fn vitri(self) -> vitri::cnf::Mode {
        match self {
            Mode::Compile => vitri::cnf::Mode::Compile,
            Mode::Mc => vitri::cnf::Mode::Mc,
            Mode::Wmc => vitri::cnf::Mode::Wmc,
            Mode::Pmc => vitri::cnf::Mode::Pmc,
            Mode::Pwmc => vitri::cnf::Mode::Pwmc,
        }
    }

    /// The mode's name, as `--mode`, the record's `mode` and the `c t` header
    /// spell it: vitri's token, so all three spell a mode the same way.
    pub(crate) fn token(self) -> &'static str {
        self.vitri().token()
    }

    /// The count one `c t` line and a `c p show` line declare: the `c t` type,
    /// made projected by a show line.
    fn from_header(header_type: Option<vitri::cnf::Mode>, show: bool) -> Option<Self> {
        let (weighted, projected) = match header_type {
            Some(header_type) => (header_type.is_weighted(), header_type.is_projected() || show),
            None if show => (false, true),
            None => return None,
        };
        Some(match (weighted, projected) {
            (true, true) => Mode::Pwmc,
            (true, false) => Mode::Wmc,
            (false, true) => Mode::Pmc,
            (false, false) => Mode::Mc,
        })
    }

    /// The mode the instance header declares, and the header lines that
    /// declare it, or `None` when it declares none.
    ///
    /// A `c t <type>` line names a count, and a `c p show` line makes the count
    /// projected; `c p weight` lines alone declare nothing. A show line without
    /// a `c t` line reads as a projected count, which `--mode pwmc` may make
    /// weighted.
    pub(crate) fn declared(meta: &vitri::cnf::CnfMeta) -> Option<(Self, String)> {
        let header_type = meta.declared_track();
        let show = meta.declared_show_vars().is_some();
        let mode = Self::from_header(header_type, show)?;
        let lines = match header_type {
            Some(header_type) if show && !header_type.is_projected() => {
                format!("the instance's `c t {}` and `c p show` lines", header_type.token())
            }
            Some(header_type) => format!("the instance's `c t {}` line", header_type.token()),
            None => "the instance's `c p show` line".to_owned(),
        };
        Some((mode, lines))
    }

    /// Merge the mode the caller asked for (`--mode`, or the `mode` argument
    /// of [`crate::count`]) with the instance header. Without one the header
    /// decides, and a header that declares nothing is a plain compile. With
    /// one the header must agree: a mode the header contradicts is a usage
    /// error naming both. A lone `c p show` line agrees with either projected
    /// mode.
    ///
    /// Asking for mode mc or pmc also refuses an instance with `c p weight`
    /// lines, which the unweighted count would ignore: a mode asked for
    /// refuses input that mode would silently drop, as mc refuses a `c p show`
    /// line. A header-driven count (`c t mc` or `c t pmc`, no mode asked for)
    /// keeps reading past the weights, and mode compile, whose circuit has no
    /// use for weights, ignores them.
    ///
    /// The messages name the mode, not the flag, so they read the same to the
    /// binary's user and to a library caller.
    pub(crate) fn resolve(flag: Option<Self>, meta: &vitri::cnf::CnfMeta) -> Result<Self, crate::Error> {
        let mode = Self::merge(flag, meta)?;
        let weighted_instead = match flag {
            Some(Mode::Mc) => "mode wmc to count with the weights (mode pwmc for a projected count)",
            Some(Mode::Pmc) => "mode pwmc to count with the weights",
            _ => return Ok(mode),
        };
        if meta.declared_weights().is_some() {
            return Err(crate::Error::Usage(format!(
                "mode {} would ignore the instance's `c p weight` lines. Ask for \
                 {weighted_instead}.",
                mode.token()
            )));
        }
        Ok(mode)
    }

    /// [`Mode::resolve`] without the weight check: the flag against the
    /// header's `c t` and `c p show` lines.
    fn merge(flag: Option<Self>, meta: &vitri::cnf::CnfMeta) -> Result<Self, crate::Error> {
        let declared = Self::declared(meta);
        let (flag, (header, lines)) = match (flag, declared) {
            (None, None) => return Ok(Mode::Compile),
            (None, Some((header, _))) => return Ok(header),
            (Some(flag), None) => return Ok(flag),
            (Some(flag), Some(declared)) => (flag, declared),
        };
        let show_only = meta.declared_track().is_none();
        if flag == header || (show_only && flag.projected()) {
            return Ok(flag);
        }
        let what = if show_only {
            "makes the count projected (mode pmc or pwmc)".to_owned()
        } else {
            format!("declares mode {}", header.token())
        };
        Err(crate::Error::Usage(format!(
            "mode {} contradicts {lines}, which {what}. Leave the mode unset to answer \
             what the header declares.",
            flag.token()
        )))
    }

    pub(crate) fn projected(self) -> bool {
        self.vitri().is_projected()
    }
}

/// The item of `all` whose `token` is `value`, or else a message that names
/// `what` and lists every token. Modes, preprocessing levels and output
/// formats are all read through it.
pub(crate) fn one_of<T: Copy>(what: &str, value: &str, all: &[T], token: fn(T) -> &'static str) -> Result<T, String> {
    all.iter()
        .copied()
        .find(|item| token(*item) == value)
        .ok_or_else(|| format!("unknown {what} {value:?}; valid: {}.", spelled(all, token, ", ")))
}

/// Every spelling of a vocabulary, in its order, joined by `separator`.
pub(crate) fn spelled<T: Copy>(all: &[T], token: fn(T) -> &'static str, separator: &str) -> String {
    all.iter().map(|item| token(*item)).collect::<Vec<_>>().join(separator)
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
