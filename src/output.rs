//! What the run answers, and what the command line prints of it.
//!
//! Each mode's runner returns what it answered as a typed value — a model
//! count with its attribution, a weighted count, a projected count with the
//! fields its stages measured, the compiled diagram — and the driver records
//! it once, here: the verdict and the count, and the run's record, [`Stats`]:
//! sizes, timings, which stage answered, the preprocessing and sat-prune
//! tallies, how the run ended. `driver::solve` hands both back as an
//! [`Answer`], which is what [`crate::count`] returns. `eprintln!` diagnostics
//! stay with the stage that emits them.
//!
//! The command line's half is [`finish`]: stdout carries the answer — the MCC
//! answer lines under `--output-format mcc`, and otherwise a plainly worded
//! count and, with `--save-tdd`, the line naming the written file — and
//! `--stats` writes the record as one JSON object, to stdout after the answer
//! when its path is `-`.
//!
//! `docs/stats.md` is the record's field table.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::cli::Cli;
use crate::problem::WeightedCount;
use crate::config::{Config, Mode, OutputFormat};
use crate::run::{Answer, Count};
use crate::Error;

/// Which stage of a model count produced the answer: the record's
/// `answered_by`, and the word the stage's stderr lines carry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum AnsweredBy {
    /// The compile answered: on the cascade, the first compile, of the Arjun
    /// reduction when one was kept; on the streaming route, its one compile.
    Compile,
    /// Preprocessing settled the count, so nothing was compiled: the formula
    /// was refuted, or no variable was left and the count is the lift.
    Preprocessing,
    /// Support enumeration, the fallback that counts the Arjun-reduced formula
    /// by enumerating assignments to its independent support.
    SupportEnumeration,
    /// The fallback that re-reduces without SBVA, then compiles again.
    NoSbvaRetry,
    /// The fallback that compiles the formula Arjun was given, unreduced.
    UnreducedRetry,
    /// Shannon split assembled the count from cofactors.
    ShannonSplit,
}

impl AnsweredBy {
    /// The token recorded in `answered_by`. Treat these as a wire format:
    /// stored sweeps are read back by them.
    pub(crate) fn token(self) -> &'static str {
        match self {
            AnsweredBy::Compile => "compile",
            AnsweredBy::Preprocessing => "preprocessing",
            AnsweredBy::SupportEnumeration => "support-enumeration",
            AnsweredBy::NoSbvaRetry => "no-sbva-retry",
            AnsweredBy::UnreducedRetry => "unreduced-retry",
            AnsweredBy::ShannonSplit => "shannon-split",
        }
    }
}

/// What became of Arjun's reduction on a run that ran Arjun: the record's
/// `reduction`, absent when Arjun did not run.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reduction {
    /// Arjun's reduction was kept, and the count went through it.
    Used,
    /// Arjun ran and no reduction was kept — it gave up inside its budget, or
    /// its result was discarded — so the count went through the formula it was
    /// given.
    Unavailable,
}

impl Reduction {
    /// The token recorded in `reduction`.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Reduction::Used => "used",
            Reduction::Unavailable => "unavailable",
        }
    }
}

// ── The record ───────────────────────────────────────────────────────────────

/// One run, as a harness or a researcher reads it back: the record the
/// binary's `--stats` writes, as a value.
///
/// Field order here is the order the JSON object carries, and the names are a
/// wire format, the keys of that object. A field is `None` when the run has
/// nothing to say about it — no nulls in the JSON, and never a `0` standing for
/// "not measured". Counts are decimal strings because they exceed 64 bits;
/// everything else is a number, a boolean or a token. Serialize it with
/// `serde_json` to get the object the binary writes.
///
/// The user guide's reference for the result record tabulates the fields and
/// which modes carry each.
#[derive(Clone, Debug, Default, Serialize)]
#[non_exhaustive]
pub struct Stats {
    /// `mc`, `wmc`, `pmc`, `pwmc`, or `compile` for a plain compile: the
    /// [`Mode`] the run resolved to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'static str>,
    /// How the run ended: `solved`, or the class of the error that stopped it
    /// (`usage`, `environment`, `stage`, `vtree-spec`, `input`, `artifact`,
    /// `incomplete`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_reason: Option<&'static str>,
    /// Variables in the parsed formula.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<u32>,
    /// Clauses in the parsed formula.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clauses: Option<u64>,
    /// The vtree construction the run compiles on: the binary's `--vtree`, or
    /// `portfolio`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vtree_spec: Option<String>,
    /// The portfolio candidate that won selection, when a portfolio ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vtree_candidate: Option<String>,

    /// Whether the formula has a model, as the run found it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub satisfiable: Option<bool>,
    /// The integer count, as a decimal string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_count: Option<String>,
    /// The weighted count, spelled as the answer line spells it: an exact
    /// rational `<n>/<d>` in the exact domain, `<mantissa>e<exponent>` in the
    /// bounded log domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weighted_count: Option<String>,
    /// log10 of the weighted count's magnitude, in the log domain only, and
    /// only when it is finite — a zero count has no logarithm and says so
    /// through `weighted_count_sign`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weighted_count_log10: Option<f64>,
    /// Sign of the weighted count in the log domain: `-1`, `0` or `1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weighted_count_sign: Option<i8>,

    /// Nodes in the compiled diagram. Only the plain compile path materializes
    /// one; the counting paths stream and never build it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tdd_nodes: Option<u64>,
    /// Node-pairs in the compiled diagram.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tdd_pairs: Option<u64>,
    /// Widest vtree level of the compiled diagram.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tdd_width: Option<u64>,
    /// Largest diagram the compile held at once, in node-pairs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_pairs: Option<u64>,

    /// Which stage of a model count produced the answer: `compile`,
    /// `preprocessing`, `support-enumeration`, `no-sbva-retry`,
    /// `unreduced-retry` or `shannon-split`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<&'static str>,
    /// What became of Arjun's reduction on a model count: `used`, or
    /// `unavailable` when Arjun ran and no reduction was kept. `None` when
    /// Arjun did not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduction: Option<&'static str>,

    /// Wall in preprocessing, in milliseconds. On the model count's cascade it
    /// is the simplify chain alone, since Arjun has its own key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preprocess_ms: Option<u64>,
    /// Wall in Arjun, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arjun_ms: Option<u64>,
    /// Wall spent building vtrees, over the whole run, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vtree_ms: Option<u64>,
    /// Wall spent compiling, in milliseconds, less every phase with its own
    /// key: preprocessing, Arjun and vtree construction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compile_ms: Option<u64>,
    /// Wall in the model count's cascade on a kept reduction: the first
    /// compile, then any fallbacks and Shannon split, vtree construction
    /// inside it included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cascade_ms: Option<u64>,
    /// Wall in backbone detection, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backbone_ms: Option<u64>,
    /// Backbone literals found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backbone_found: Option<u64>,
    /// SAT probes backbone detection spent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backbone_probes: Option<u64>,
    /// Variables in the show set of the formula the projected count was
    /// answered on, after preprocessing; `None` without a show set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_vars: Option<u64>,

    /// The record's `sat_prune`: `off` when `TIDIDI_SAT_PRUNE=off` switched
    /// the sat-prune pass off for the run, a diagnostic for paired on/off
    /// measurements. Absent when the pass is on, the default.
    #[serde(rename = "sat_prune", skip_serializing_if = "Option::is_none")]
    pub sat_prune_switch: Option<&'static str>,
    /// Why the sat-prune pass stood down, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sat_prune_disabled_by: Option<&'static str>,
    /// The sat-prune tallies, keyed `sat_prune_<tally>`, each present only when
    /// it is nonzero. Times end `_us` and are microseconds, the unit the pass
    /// accumulates them in. The JSON object carries them as top-level keys.
    #[serde(flatten)]
    pub sat_prune: BTreeMap<&'static str, u64>,
}

/// What a mode answered: whether the formula has a model, its count, and in
/// the compile mode the diagram the count was read off.
pub(crate) struct Verdict {
    pub(crate) satisfiable: bool,
    pub(crate) count: Count,
    pub(crate) diagram: Option<tididi::Tdd>,
}

/// Open the run's record: the facts fixed before the mode runs.
///
/// Called once, from the driver, at the point the mode and the vtree
/// construction are both resolved — which is the last point every mode passes
/// through. A run refused before then (bad argv, an instance the build will not
/// answer) has no record; its stderr diagnostic is the whole story.
pub(crate) fn open_record(config: &Config, formula: &vitri::cnf::CnfFormula) -> Stats {
    Stats {
        mode: Some(config.mode.token()),
        variables: Some(formula.num_vars()),
        clauses: Some(formula.clauses().len() as u64),
        vtree_spec: Some(config.vtree_spec.clone()),
        sat_prune_switch: (!config.sat_prune).then_some("off"),
        ..Stats::default()
    }
}

/// The error class, as the record names it.
pub(crate) fn exit_reason<T>(outcome: &Result<T, Error>) -> &'static str {
    match outcome {
        Ok(_) => "solved",
        Err(Error::Usage(_)) => "usage",
        Err(Error::Environment(_)) => "environment",
        Err(Error::Stage(_)) => "stage",
        Err(Error::VtreeSpec(_)) => "vtree-spec",
        Err(Error::Input(_)) => "input",
        Err(Error::Artifact(_)) => "artifact",
        Err(Error::Incomplete(_)) => "incomplete",
    }
}

/// Close the run's record once its mode has returned.
///
/// The last three facts land here rather than with the answer, because they
/// are the run's and not any one stage's: the vtree accumulator's total, the
/// sat-prune tallies of every compile in the run, `sat_prune`, and how the run
/// ended. Reading each in one place is also what keeps a give-up row carrying
/// them.
pub(crate) fn close_record(
    stats: &mut Stats,
    outcome: &Result<Verdict, Error>,
    sat_prune: Option<crate::compile::sat_prune::SatPruneStats>,
) {
    stats.exit_reason = Some(exit_reason(outcome));
    // A stage can finish after the whole-run deadline, and its answer reach
    // the record before the driver rejects it. Keep diagnostics, never that
    // answer.
    if outcome.is_err() {
        stats.satisfiable = None;
        stats.model_count = None;
        stats.weighted_count = None;
        stats.weighted_count_log10 = None;
        stats.weighted_count_sign = None;
        stats.answered_by = None;
    }
    stats.vtree_ms = crate::budget::vtree_build_ms().map(|ms| ms as u64);
    // Each tally is recorded only when it is nonzero: presence is the "this
    // happened" signal. None is there when no pass ran to its end.
    if let Some(prune) = sat_prune {
        stats.sat_prune = prune.tallies().filter(|(_, v)| *v > 0).collect();
        stats.sat_prune_disabled_by = prune.disabled_by.map(crate::compile::sat_prune::PruneStop::key);
    }
}

// ── The answers ──────────────────────────────────────────────────────────────

/// Write the verdict and the count into the record, as the count's domain
/// spells it, and hand the verdict back.
fn recorded(stats: &mut Stats, satisfiable: bool, count: Count, diagram: Option<tididi::Tdd>) -> Verdict {
    stats.satisfiable = Some(satisfiable);
    match &count {
        Count::Integer(n) => stats.model_count = Some(n.to_string()),
        Count::Rational(_) => stats.weighted_count = Some(count.to_string()),
        Count::Log(s) => {
            // A zero count has no logarithm; the sign says so instead.
            let log10 = s.log10_abs();
            stats.weighted_count_log10 = log10.is_finite().then_some(log10);
            stats.weighted_count_sign = Some(s.sign);
            stats.weighted_count = Some(count.to_string());
        }
    }
    Verdict { satisfiable, count, diagram }
}

/// The portfolio candidate that won, recorded for the integer and weighted
/// counts and the compile, since only by then has selection run. The spec
/// itself is settled before the mode dispatches and is recorded once, in
/// [`open_record`].
fn record_vtree_candidate(stats: &mut Stats, config: &Config) {
    if let Some(candidate) = config.portfolio_history.last_winning_spec() {
        stats.vtree_candidate = Some(candidate);
    }
}

/// Record an integer count with what every integer mode reports alongside it;
/// a model count is zero exactly when the formula has no model, so the count
/// is the verdict.
fn record_integer(
    stats: &mut Stats,
    config: &Config,
    count: num_bigint::BigUint,
    peak_pairs: usize,
    preprocessing: PreprocessReport,
    compile_ms: u128,
    diagram: Option<tididi::Tdd>,
) -> Verdict {
    record_vtree_candidate(stats, config);
    if peak_pairs > 0 {
        stats.peak_pairs = Some(peak_pairs as u64);
    }
    preprocessing.write(stats);
    stats.compile_ms = Some(compile_ms as u64);
    let satisfiable = count != num_bigint::BigUint::ZERO;
    recorded(stats, satisfiable, Count::Integer(count), diagram)
}

/// What preprocessing did, in the four numbers the result record keeps. The
/// modes carry them and read none of them: they travel from the frontend that
/// measured them to the record.
#[derive(Clone, Copy)]
pub(crate) struct PreprocessReport {
    pub(crate) preprocess_ms: u64,
    pub(crate) backbone_ms: u64,
    pub(crate) backbone_found: u64,
    pub(crate) backbone_probes: u64,
}

impl PreprocessReport {
    /// The report of one Vitri run, its whole wall counted as preprocessing.
    pub(crate) fn of(telemetry: &vitri::bundle::PreprocessTelemetry) -> Self {
        PreprocessReport {
            preprocess_ms: telemetry.total_ms,
            backbone_ms: telemetry.backbone_ms.unwrap_or(0),
            backbone_found: telemetry.backbone_found as u64,
            backbone_probes: telemetry.backbone_probes as u64,
        }
    }

    fn write(self, stats: &mut Stats) {
        stats.preprocess_ms = Some(self.preprocess_ms);
        stats.backbone_ms = Some(self.backbone_ms);
        stats.backbone_found = Some(self.backbone_found);
        stats.backbone_probes = Some(self.backbone_probes);
    }
}

/// Which stage produced a model count, with the Arjun and cascade walls the
/// record keeps beside it.
#[derive(Clone, Copy)]
pub(crate) struct Attribution {
    /// Not an `Option`: every caller must name the stage, so a new answer
    /// site cannot be added without saying which stage answers there.
    pub(crate) answered_by: AnsweredBy,
    /// `None` when Arjun did not run.
    pub(crate) reduction: Option<Reduction>,
    pub(crate) arjun_ms: Option<u128>,
    pub(crate) cascade_ms: Option<u128>,
}

/// A model count, the cascade's or the streaming route's, with what the record
/// keeps beside it. Nothing is materialized as a diagram, so there are no
/// diagram sizes beyond the compile peak.
pub(crate) struct McAnswer {
    pub(crate) count: num_bigint::BigUint,
    pub(crate) peak_pairs: usize,
    pub(crate) preprocessing: PreprocessReport,
    pub(crate) compile_ms: u128,
    pub(crate) attribution: Attribution,
}

/// The verdict and the record of a model count.
pub(crate) fn record_mc(stats: &mut Stats, config: &Config, answer: McAnswer) -> Verdict {
    let McAnswer { count, peak_pairs, preprocessing, compile_ms, attribution } = answer;
    stats.answered_by = Some(attribution.answered_by.token());
    stats.reduction = attribution.reduction.map(Reduction::token);
    stats.arjun_ms = attribution.arjun_ms.map(|ms| ms as u64);
    stats.cascade_ms = attribution.cascade_ms.map(|ms| ms as u64);
    record_integer(stats, config, count, peak_pairs, preprocessing, compile_ms, None)
}

/// The verdict and the record of a weighted count.
///
/// The count is `sign·10^(log10_abs)` in the bounded log domain, which every
/// weighted count runs in (`src/stage/wmc.rs`), spelled in scientific notation
/// ([`Count::Log`]), because the magnitude can be astronomically large or
/// small and a plain f64 would overflow or underflow.
pub(crate) fn record_wmc(
    stats: &mut Stats,
    config: &Config,
    count: WeightedCount<tididi::diagram::WeightValue>,
) -> Verdict {
    use tididi::diagram::SignedLog;
    record_vtree_candidate(stats, config);
    let (satisfiable, s) = match count {
        WeightedCount::Unsatisfiable => (false, SignedLog::zero()),
        WeightedCount::Satisfiable(value) => {
            (true, *value.as_log().expect("the weighted count runs in the log domain"))
        }
    };
    recorded(stats, satisfiable, Count::Log(s), None)
}

/// A projected count, PMC's or PWMC's, with the two record fields its stages
/// measure; [`crate::stage::run_stages`] fills it at the answer.
pub(crate) struct Projected<V> {
    pub(crate) value: V,
    /// Variables in the show set of the problem the answering stage was
    /// handed; `None` when there is no show set.
    pub(crate) show_vars: Option<usize>,
    /// The stages' wall, less the rewrites and the vtree construction.
    pub(crate) compile_ms: u128,
}

impl<V> Projected<V> {
    fn record(self, stats: &mut Stats) -> V {
        stats.compile_ms = Some(self.compile_ms as u64);
        stats.show_vars = self.show_vars.map(|n| n as u64);
        self.value
    }
}

/// The verdict and the record of a projected integer (PMC) count.
pub(crate) fn record_pmc(stats: &mut Stats, answer: Projected<num_bigint::BigUint>) -> Verdict {
    let count = answer.record(stats);
    let satisfiable = count != num_bigint::BigUint::ZERO;
    recorded(stats, satisfiable, Count::Integer(count), None)
}

/// The verdict and the record of a projected weighted (PWMC) count, an exact
/// rational.
pub(crate) fn record_pwmc(
    stats: &mut Stats,
    answer: Projected<WeightedCount<num_rational::BigRational>>,
) -> Verdict {
    use num_traits::Zero;
    let (satisfiable, value) = match answer.record(stats) {
        WeightedCount::Unsatisfiable => (false, num_rational::BigRational::zero()),
        WeightedCount::Satisfiable(value) => (true, value),
    };
    recorded(stats, satisfiable, Count::Rational(value), None)
}

/// The plain compile's diagram, its count, and what the record keeps beside
/// them.
pub(crate) struct CompileAnswer {
    pub(crate) tdd: tididi::Tdd,
    /// The diagram's model count, over every original variable.
    pub(crate) count: num_bigint::BigUint,
    pub(crate) peak_pairs: usize,
    pub(crate) preprocessing: PreprocessReport,
    pub(crate) compile_ms: u128,
}

/// The verdict and the record of the plain compile: the count, and the
/// diagram it was read off, which the answer carries.
pub(crate) fn record_compile(stats: &mut Stats, config: &Config, answer: CompileAnswer) -> Verdict {
    let CompileAnswer { tdd, count, peak_pairs, preprocessing, compile_ms } = answer;
    stats.tdd_width = Some(tdd.max_width() as u64);
    stats.tdd_pairs = Some(tdd.pair_count() as u64);
    stats.tdd_nodes = Some(tdd.node_count() as u64);
    record_integer(stats, config, count, peak_pairs, preprocessing, compile_ms, Some(tdd))
}

// ── The command line ─────────────────────────────────────────────────────────

/// Print what the run answered as the command line asked, write the circuit
/// under `--save-tdd` and the record under `--stats`, and return how the run
/// ended.
///
/// Stdout, in order: the answer, or `s UNKNOWN` for a give-up under
/// `--output-format mcc`; the `tdd written to` line; the record, when
/// `--stats -`. A run that failed before opening its record writes none. An
/// unwritable record fails a run that otherwise succeeded, because the
/// operator asked for the artifact.
pub(crate) fn finish(solved: Result<Answer, crate::driver::Failure>, cli: &Cli) -> Result<(), Error> {
    let mcc = cli.output_format == OutputFormat::Mcc;
    let (outcome, stats) = match solved {
        Ok(answer) => {
            print_answer(&answer, mcc);
            let saved = match &cli.save_tdd_path {
                Some(path) => {
                    // The driver refuses `--save-tdd` on every mode but the
                    // compile, and the compile always answers with its diagram.
                    let tdd = answer.diagram.as_ref().expect("a compile answers with its diagram");
                    save_diagram(tdd, path, mcc)
                }
                None => Ok(()),
            };
            let mut stats = answer.stats;
            // A circuit that could not be written ends the run that computed it.
            stats.exit_reason = Some(exit_reason(&saved));
            (saved, Some(stats))
        }
        Err(crate::driver::Failure { error, stats }) => {
            let outcome = Err(error);
            report_give_up(&outcome, mcc);
            (outcome, stats.map(|stats| *stats))
        }
    };
    let written = match (&cli.stats_path, stats) {
        (Some(path), Some(stats)) => write_record(&stats, path),
        _ => Ok(()),
    };
    outcome.and(written)
}

/// Write the record as one JSON object at `path`, or on stdout for `-`.
fn write_record(stats: &Stats, path: &str) -> Result<(), Error> {
    let json = serde_json::to_string(stats)
        .map_err(|error| Error::Artifact(format!("could not build the stats record: {error}")))?;
    if path == "-" {
        println!("{json}");
        return Ok(());
    }
    std::fs::write(path, format!("{json}\n"))
        .map_err(|error| Error::Artifact(format!("could not write {path}: {error}")))
}

/// Write the compiled circuit to `path` and its vtree to `<path>.vtree`, and
/// say so on stdout under the plain output format.
fn save_diagram(tdd: &tididi::diagram::Tdd, path: &str, mcc: bool) -> Result<(), Error> {
    // The `.tdd` format labels vtree nodes but does not describe the tree,
    // and `tididi::io::load_tdd` takes the vtree as an argument, so the
    // vtree goes beside the circuit in its own text format.
    let vtree_path = format!("{path}.vtree");
    tididi::io::save_tdd(tdd, path)
        .map_err(|error| Error::Artifact(format!("could not write the diagram to {path}: {error}")))?;
    std::fs::write(&vtree_path, tdd.vtree().to_text())
        .map_err(|error| Error::Artifact(format!("could not write the vtree to {vtree_path}: {error}")))?;
    if !mcc {
        println!(
            "tdd written to {path}, vtree to {vtree_path}: {} nodes, {} pairs",
            tdd.node_count(),
            tdd.pair_count(),
        );
    }
    Ok(())
}

/// log10 of a non-negative big integer to ~double precision, for the MCC
/// `c s log10-estimate` line. Returns `-inf` for zero, exactly as the
/// MC-Competition output spec (v1.2 §3) requires when the count is 0.
fn log10_biguint(n: &num_bigint::BigUint) -> f64 {
    if *n == num_bigint::BigUint::ZERO {
        return f64::NEG_INFINITY;
    }
    // Keep the top ~53 bits (one f64 mantissa) and account for the rest via
    // the binary exponent: log10(n) = log10(n >> s) + s*log10(2). This stays
    // accurate for counts far larger than f64's range (2^1024).
    let bits = n.bits(); // index of the MSB + 1, >= 1
    let shift = (bits as usize).saturating_sub(53);
    let high = (n >> shift).to_u64_digits().first().copied().unwrap_or(1) as f64;
    high.log10() + (shift as f64) * std::f64::consts::LOG10_2
}

/// The competition's status line. "The solver may output `UNKNOWN`, but is not
/// allowed to output any other value than these three": `None` is a run that
/// gave up.
fn status_line(satisfiable: Option<bool>) -> &'static str {
    match satisfiable {
        Some(true) => "s SATISFIABLE",
        Some(false) => "s UNSATISFIABLE",
        None => "s UNKNOWN",
    }
}

/// The MCC answer to a run that gave up: `s UNKNOWN` under
/// `--output-format mcc`, the one status line the format allows without a
/// count.
///
/// Every task's give-up reaches the driver as [`Error::Incomplete`], after the
/// stage that gave up said why on stderr, so [`finish`] prints this once
/// rather than each stage printing it. A run stopped by any other error has
/// refused the instance or failed, and prints nothing on stdout.
fn report_give_up(outcome: &Result<(), Error>, mcc: bool) {
    if matches!(outcome, Err(Error::Incomplete(_))) && mcc {
        println!("{}", status_line(None));
    }
}

/// Print one answer.
///
/// Under `--output-format mcc` it is the four lines of the Model Counting
/// Competition's output format (data format v1.2 §3), in the format's order,
/// for every task:
///
/// ```text
/// s SATISFIABLE | s UNSATISFIABLE
/// c s type mc | wmc | pmc | pwmc
/// c s log10-estimate <log10 of the count> | c s neglog10-estimate <log10 of -count>
/// c s <solver type> <precision> <notation> <count>
/// ```
///
/// and otherwise one plainly worded line. This is the only printer of either,
/// so no task can drift from the order or the spelling. The status line is the
/// solver's verdict, not the count's: a zero weight, or weights that cancel,
/// make the weighted count of a satisfiable formula zero.
fn print_answer(answer: &Answer, mcc: bool) {
    let count = &answer.count;
    if mcc {
        let (log10, negative) = magnitude(count);
        let estimate = if negative { "neglog10-estimate" } else { "log10-estimate" };
        println!("{}", status_line(Some(answer.satisfiable)));
        println!("c s type {}", task(answer.mode));
        println!("c s {estimate} {log10}");
        println!("c s {}", exact(count));
    } else {
        println!("{}: {count}", plain_label(answer.mode));
    }
}

/// The task as `c s type` names it. The plain compile's count is a model count
/// too, read off the diagram over every original variable, so it answers as
/// `mc`.
fn task(mode: Mode) -> &'static str {
    match mode {
        Mode::Compile => Mode::Mc.token(),
        mode => mode.token(),
    }
}

/// The words before the count on the plain answer line.
fn plain_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Compile | Mode::Mc => "model count",
        Mode::Wmc => "weighted count",
        Mode::Pmc => "projected model count",
        Mode::Pwmc => "projected weighted count",
    }
}

/// log10 of the count's magnitude, `-inf` for a zero count, printed in full
/// double precision; and whether the count is below zero, which only negative
/// weights make it.
fn magnitude(count: &Count) -> (f64, bool) {
    match count {
        Count::Integer(n) => (log10_biguint(n), false),
        Count::Rational(r) => (
            log10_biguint(r.numer().magnitude()) - log10_biguint(r.denom().magnitude()),
            r.numer().sign() == num_bigint::Sign::Minus,
        ),
        Count::Log(s) => (s.log10_abs(), s.sign < 0),
    }
}

/// The last MCC answer line after its `c s `: solver type, precision, notation
/// and the count, as each task has always printed it.
fn exact(count: &Count) -> String {
    match count {
        Count::Integer(n) => format!("exact arb int {n}"),
        // The standard MCC weighted-float answer: "exact" is the solver type,
        // and `double` says the value is bounded-precision f64.
        Count::Log(_) => format!("exact double prec-sci {count}"),
        Count::Rational(_) => format!("exact arb frac {count}"),
    }
}

#[cfg(test)]
#[path = "tests/output.rs"]
mod tests;
