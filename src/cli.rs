//! The command line: argv to [`Cli`], the run it asks for and where its answer
//! goes, and [`run_cli`], which makes that run and prints its answer.
//!
//! Flag parsing and the usage text. The mode, the limits and the settings go
//! to `driver::solve`, the run the library's [`crate::count`] also makes,
//! which refuses a setting the mode it resolves would ignore
//! (`Settings::refuse_inert`); the rest says how `output` prints what it
//! answered.

use std::path::PathBuf;

use crate::config::{one_of, spelled, Limits, Mode, PreprocessLevel, Settings};
use crate::Error;

/// The address-space limit, in MiB, that `--mem-limit-mb` sets when it is
/// absent, so a run without the flag still arms the memory-adaptive stages.
const DEFAULT_MEM_LIMIT_MB: u64 = 30_000;

/// How stdout spells the answer (`--output-format`).
///
/// Set only by the flag, never by the instance's `c t` header: every
/// MCC-format instance carries one, so a header-driven switch would take the
/// plain answer line away from every such run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    /// One plainly worded line, such as `model count: 7`.
    Plain,
    /// The answer lines of the MCC output format.
    Mcc,
}

impl OutputFormat {
    /// Every format, in the order `--help` lists them.
    const ALL: [OutputFormat; 2] = [OutputFormat::Plain, OutputFormat::Mcc];

    /// The format's name, as `--output-format` spells it.
    fn token(self) -> &'static str {
        match self {
            OutputFormat::Plain => "plain",
            OutputFormat::Mcc => "mcc",
        }
    }
}

/// Run the `tididi` command line: parse the arguments, run the mode they
/// select, and print its answer and its record. A run that fails prints its
/// error on stderr after `error: `; the status is the one its class sets
/// (`Error::exit_code`).
pub fn run_cli() -> std::process::ExitCode {
    let outcome = parse_args().and_then(|request| match request {
        Request::Print(text) => crate::output::print(&text),
        Request::Run(cli) => {
            let input = crate::Input::File(cli.path.clone());
            let solved = crate::driver::solve(input, cli.mode, &cli.limits, &cli.settings);
            crate::output::finish(solved, &cli)
        }
    });
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(error.exit_code())
        }
    }
}

/// What a command line asks for: a run, or a text to print and nothing else
/// (`--help`, `--version`).
enum Request {
    Run(Cli),
    Print(String),
}

/// One command line, parsed: the run it asks for and where its answer goes.
pub(crate) struct Cli {
    /// The DIMACS file.
    pub(crate) path: PathBuf,
    /// `--mode`, when it was on the command line.
    pub(crate) mode: Option<Mode>,
    /// `--time-limit-ms` (or `TIDIDI_TIME_LIMIT_MS`) and `--mem-limit-mb`.
    pub(crate) limits: Limits,
    /// `--vtree`, `--preprocess`, whether `--save-tdd` was given, and
    /// `TIDIDI_SAT_PRUNE`.
    pub(crate) settings: Settings,
    /// How stdout spells the answer (`--output-format`).
    pub(crate) output_format: OutputFormat,
    /// `--stats <path>`: write the run's stats record — mode, counts, sizes,
    /// per-stage timings, the preprocessing and SAT-pruning tallies, how the run
    /// ended — as one JSON object at `<path>`, or to stdout after the answer
    /// when the path is `-`. Valid in every mode.
    /// `docs/stats.md` is the field table.
    pub(crate) stats_path: Option<PathBuf>,
    /// `--save-tdd <path>`: where the compiled circuit goes, and its vtree
    /// beside it at `<path>.vtree`.
    pub(crate) save_tdd_path: Option<PathBuf>,
}

/// The usage text: the flags, the exit statuses, and the vtree specs vitri's
/// grammar accepts.
fn usage(prog: &str) -> String {
    let modes = spelled(&Mode::ALL, Mode::token, "|");
    let levels = spelled(&PreprocessLevel::ALL, PreprocessLevel::token, "|");
    let formats = spelled(&OutputFormat::ALL, OutputFormat::token, "|");
    let mut text = format!(
        "Usage: {prog} [options] <dimacs-file>

Compiles a CNF formula in DIMACS form to a tree decision diagram, or counts its
models.

Options:
  --mode <{modes}>
                       What to compute: the compiled circuit and its model count
                       (compile), the model count (mc), the weighted model count
                       (wmc), or the model count (pmc) or weighted model count
                       (pwmc) projected onto the `c p show` variables. Default:
                       the mode the instance header declares (a `c t` line, made
                       projected by a `c p show` line), else compile. A mode the
                       header contradicts is refused, as is mc or pmc on an
                       instance with `c p weight` lines.
  --vtree <spec>       Compile on this vtree (specs below). Without it, compile,
                       pmc and pwmc use portfolio, and mc and wmc run the counting
                       cascade: Arjun reduces the formula, portfolio picks a vtree
                       for the reduced formula, and fallbacks follow if that
                       compile gives up. With it, mc compiles on <spec> without
                       the reduction, and wmc refuses it.
  --preprocess <{levels}>
                       Simplify the formula before the compile (full, the
                       default) or not (none). Applies to compile, mc and wmc;
                       pmc and pwmc refuse it, since their projection stages
                       always simplify.
  --save-tdd <file>    Write the compiled circuit to <file> and its vtree to
                       <file>.vtree (compile mode only); the library's load_tdd
                       reads the circuit back over that vtree.
  --output-format <{formats}>
                       How stdout spells the answer: one plain line (plain, the
                       default), or the MCC output format's answer lines (s,
                       c s type, c s log10-estimate, c s exact), with `s UNKNOWN`
                       when the run gives up (mcc).
  --stats <path>       Write the run's stats record (mode, counts, sizes, per-stage
                       timings, preprocessing and SAT-pruning tallies, exit reason)
                       as one JSON object at <path>. `-` writes it to stdout after
                       the answer.
  --time-limit-ms <ms> Time limit for the whole run, in milliseconds. Every stage
                       budget is a share of it; without it the stages use fixed
                       defaults. Env: TIDIDI_TIME_LIMIT_MS (the flag wins).
  --mem-limit-mb <MiB> Address-space limit (RLIMIT_AS) to set at startup, as
                       `ulimit -v` does; the memory-adaptive stages read it back.
                       It only lowers the limit, and 0 leaves it unchanged.
                       Default: {DEFAULT_MEM_LIMIT_MB}.
  -h, --help           Print this text and exit
  -V, --version        Print the version and exit

Exit status: 0 with an answer; 1 without one (the run gave up), or for an input
it cannot read, a vtree spec it cannot use or an output it cannot write; 2 for a
usage error, a refused instance or environment setting, or a failed stage.

Vtree specs:
"
    );
    for base in vitri::spec::vtree_spec_bases() {
        let params = vitri::spec::spec_param_docs(&base);
        if params.is_empty() {
            text.push_str(&format!("  {base}\n"));
        } else {
            let keys = params
                .iter()
                .map(|param| format!("{}={}", param.key, param.values))
                .collect::<Vec<_>>()
                .join(",");
            text.push_str(&format!("  {base}[:{keys}]\n"));
        }
    }
    text
}

fn parse_args() -> Result<Request, Error> {
    let mut args = std::env::args_os();
    let prog = args.next().map_or_else(|| "tididi".to_owned(), |prog| prog.to_string_lossy().into_owned());
    // A command line the parser cannot read is a usage error: it names what
    // is wrong and points to the usage, and the binary exits 2.
    let usage_error = |what: String| {
        Error::Usage(format!(
            "{what}\nUsage: {prog} [options] <dimacs-file> (`{prog} --help` lists the options)"
        ))
    };

    let mut vtree_spec: Option<String> = None;
    let mut dimacs_path: Option<PathBuf> = None;
    let mut save_tdd_path: Option<PathBuf> = None;
    // `--stats <path>`: where the run's stats record goes. Valid in every
    // mode, so it has no inert-combination guard of its own.
    let mut stats_path: Option<PathBuf> = None;
    // `--time-limit-ms`; `TIDIDI_TIME_LIMIT_MS` fills it in below when the
    // flag is absent.
    let mut time_limit_ms: Option<u64> = None;
    let mut output_format = OutputFormat::Plain;
    let mut mode_flag: Option<Mode> = None;
    // RLIMIT_AS soft limit to self-impose at startup (MiB).
    let mut mem_limit_mb = DEFAULT_MEM_LIMIT_MB;
    // `--preprocess`; the run's default, full, applies without it.
    let mut preprocess: Option<PreprocessLevel> = None;
    // The flags read so far.
    let mut given: Vec<String> = Vec::new();

    while let Some(arg) = args.next() {
        // Anything that is not a flag is the input file, taken as the system
        // gave it: a path need not be text.
        if !arg.as_encoded_bytes().starts_with(b"-") {
            if let Some(first) = &dimacs_path {
                return Err(usage_error(format!(
                    "a second input file {arg:?} after {first:?}; tididi reads one DIMACS file."
                )));
            }
            dimacs_path = Some(arg.into());
            continue;
        }
        let flag = arg.to_string_lossy();
        // Each flag is read once: a second value would replace the first
        // without a word, as a setting the run ignores.
        if given.iter().any(|seen| *seen == flag) {
            return Err(usage_error(format!("{flag} is given twice; a run takes one value for each flag.")));
        }
        given.push(flag.clone().into_owned());
        // The argument after the flag: as given for a path, and otherwise
        // read as text.
        let mut value = || args.next().ok_or_else(|| usage_error(format!("{flag} needs a value.")));
        let text = |value: std::ffi::OsString| {
            value.into_string().map_err(|value| usage_error(format!("{flag} {value:?} is not text.")))
        };
        match flag.as_ref() {
            "--vtree" => vtree_spec = Some(text(value()?)?),
            "--save-tdd" => save_tdd_path = Some(value()?.into()),
            "--stats" => stats_path = Some(value()?.into()),
            "--mode" => {
                mode_flag = Some(one_of("--mode", &text(value()?)?, &Mode::ALL, Mode::token).map_err(usage_error)?);
            }
            "--output-format" => {
                output_format = one_of("--output-format", &text(value()?)?, &OutputFormat::ALL, OutputFormat::token)
                    .map_err(usage_error)?;
            }
            "--mem-limit-mb" => {
                let val = text(value()?)?;
                mem_limit_mb = val.parse().map_err(|_| {
                    usage_error(format!(
                        "--mem-limit-mb {val:?} is not a whole number of MiB (0 leaves RLIMIT_AS untouched)."
                    ))
                })?;
            }
            "--time-limit-ms" => {
                let val = text(value()?)?;
                time_limit_ms = Some(val.parse().map_err(|_| {
                    usage_error(format!("--time-limit-ms {val:?} is not a whole number of milliseconds."))
                })?);
            }
            "--preprocess" => {
                preprocess = Some(
                    one_of("--preprocess", &text(value()?)?, &PreprocessLevel::ALL, PreprocessLevel::token)
                        .map_err(usage_error)?,
                );
            }
            // Help and the version are answers of their own, and the rest of
            // the command line is not read.
            "--help" | "-h" => return Ok(Request::Print(usage(&prog))),
            "--version" | "-V" => return Ok(Request::Print(format!("tididi {}\n", env!("CARGO_PKG_VERSION")))),
            other => return Err(usage_error(format!("unknown flag {other}."))),
        }
    }

    let dimacs_path = dimacs_path.ok_or_else(|| usage_error("no DIMACS file given.".to_owned()))?;

    // The flag wins over `TIDIDI_TIME_LIMIT_MS`, but a set value that does not
    // parse is refused either way: it is an invalid setting, and ignoring it
    // would run with no time limit at all.
    if let Some(value) = std::env::var_os("TIDIDI_TIME_LIMIT_MS") {
        let value = value.to_string_lossy();
        let env_ms = value.parse::<u64>().map_err(|_| {
            Error::Environment(format!(
                "environment variable TIDIDI_TIME_LIMIT_MS: {value:?} is not a whole \
                 number of milliseconds"
            ))
        })?;
        time_limit_ms = time_limit_ms.or(Some(env_ms));
    }

    // `TIDIDI_SAT_PRUNE=off` switches the in-compile SAT pruning off for the
    // run, a diagnostic for paired on/off measurements; `on` is the default.
    // This is the variable's one reader. Any other value is refused rather
    // than read as either, and `Settings::refuse_inert` refuses the variable,
    // set either way, in the mode whose compiles never prune.
    let sat_prune = match std::env::var_os("TIDIDI_SAT_PRUNE") {
        None => None,
        Some(value) => Some(match value.to_str() {
            Some("on") => true,
            Some("off") => false,
            _ => {
                return Err(Error::Environment(format!(
                    "environment variable TIDIDI_SAT_PRUNE: {:?} is not on or off",
                    value.to_string_lossy()
                )));
            }
        }),
    };

    Ok(Request::Run(Cli {
        path: dimacs_path,
        mode: mode_flag,
        limits: Limits { time_limit_ms, mem_limit_mb },
        settings: Settings {
            vtree: vtree_spec,
            preprocess,
            save_tdd: save_tdd_path.is_some(),
            sat_prune,
        },
        output_format,
        stats_path,
        save_tdd_path,
    }))
}
