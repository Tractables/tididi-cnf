//! One run, start to finish: read the instance, resolve the mode, and run it.
//!
//! `solve` is the run. The library's [`crate::count`] calls it with the
//! caller's mode and limits, and the command line, `cli::run_cli`, with what
//! argv set. The phases here are the run's spine — its per-run state, reading
//! the CNF or a vitri bundle (`bundle.rs`) and the plain compile route — and
//! the mode dispatch that picks between that route and the stages in
//! `crate::stage`. The stage bodies are not here.

mod bundle;

use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tididi::Engine;
use tididi::Tdd;
use tididi::vtree::Vtree;
use vitri::cnf::CnfFormula;

use crate::compile::{CompileOptions, CompileRun, Compiled, ComponentPlan, GiveUp, compile_with};
use crate::compile::sat_prune::SatPrune;
use crate::config::{Config, Limits, Mode, Settings};
use crate::frontend::CompileFrontendAttempt;
use crate::frontend::record::Outcome;
use crate::problem::Domain;
use crate::run::{Answer, Failure, Input};
use crate::Error;
use crate::output::{
    AnsweredBy, CompileAnswer, PreprocessReport, Stats, Verdict, record_compile, record_mc, record_pmc,
    record_pwmc, record_wmc,
};
use crate::stage::{
    RETRY_VTREE_SPEC, Run, run_mc_bundle, run_mc_count, run_mc_streaming, run_pmc_bundle, run_pmc_count,
    run_pwmc_bundle, run_pwmc_count, run_wmc_bundle, run_wmc_count,
};
use crate::vtree::{VtreeBuild, VtreeOptions, build_vtree_with_components, spec_family};

/// Read the DIMACS file at `path` through the one reader
/// ([`crate::parse_dimacs`] → vitri), so a run accepts and refuses exactly
/// what vitri does. Its error already names the offending line and what is
/// wrong with it; the path is prefixed here.
fn read_dimacs(path: &Path) -> Result<(CnfFormula, vitri::cnf::CnfMeta), Error> {
    let shown = path.display();
    let file = std::fs::File::open(path)
        .map_err(|error| Error::Input(format!("could not open {shown}: {error}")))?;
    eprintln!("[input] reading {shown}");
    crate::parse_dimacs(BufReader::new(file))
        .map_err(|error| Error::Input(format!("could not parse {shown}: {error}")))
}

/// The instance as [`parse_input`] read it, and the run's settings for it.
struct Parsed {
    formula: CnfFormula,
    /// The header (`c t` / `c p show` / `c p weight`); `CnfMeta::default()`
    /// for a CNF without one.
    meta: vitri::cnf::CnfMeta,
    /// A bundle's `preprocess.json`, `None` for any other input.
    record: Option<vitri::bundle::PreprocessRecord>,
    config: Config,
    program_deadline: Option<Instant>,
}

/// Start the run's clock, read the instance, resolve the mode, and refuse a
/// setting that mode, or a bundle, makes inert. A directory is read as a
/// vitri bundle: the formula is its `reduced.cnf`, the record its
/// `preprocess.json`, and the config carries its vtree
/// (`Config::input_vtree`).
fn parse_input(
    input: Input,
    mode_flag: Option<Mode>,
    limits: &Limits,
    settings: &Settings,
    vitri: vitri::RunConfig,
) -> Result<Parsed, Error> {
    // Validate an explicit spec here, the one point every compile path passes
    // through: some paths never construct a vtree from it and would otherwise
    // ignore a bad one.
    if let Some(spec) = &settings.vtree {
        crate::vtree::validate_vtree_spec(spec)?;
    }

    // The time limit is the total wall from this point.
    let program_start = Instant::now();
    let program_deadline: Option<Instant> = limits
        .time_limit_ms
        .map(|ms| program_start + std::time::Duration::from_millis(ms));

    let (formula, meta, bundle) = match input {
        Input::File(path) if path.is_dir() => {
            let bundle::Bundle { reduced, meta, record, vtree } = bundle::read_bundle(&path)?;
            (reduced, meta, Some((record, vtree)))
        }
        Input::File(path) => {
            let (formula, meta) = read_dimacs(&path)?;
            (formula, meta, None)
        }
        Input::Formula(formula, meta) => {
            // A caller can pair a formula with metadata built for another
            // variable space. Rebuild through vitri's range gate before any
            // stage reads the show set or resolves the weights.
            let meta = vitri::cnf::CnfMeta::from_parts(
                formula.num_vars(),
                meta.declared_track(),
                meta.declared_show_vars().cloned(),
                meta.declared_weights().cloned(),
            ).map_err(|error| Error::Input(error.to_string()))?;
            (formula, meta, None)
        }
    };

    // `--mode` and the instance header (`c t`, `c p show`), or a bundle's
    // record, decide the mode; this is the one place it is decided.
    let mode = match &bundle {
        Some((record, _)) => {
            let mode = bundle::bundle_mode(mode_flag, record)?;
            eprintln!("[input] a vitri bundle, preprocessed for mode {mode}");
            mode
        }
        None => Mode::resolve(mode_flag, &meta)?,
    };
    settings.refuse_inert(mode_flag, &meta, mode, bundle.is_some())?;

    eprintln!(
        "[input] {} variables, {} clauses",
        formula.num_vars(),
        formula.clauses().len()
    );

    let (record, input_vtree) = bundle.unzip();
    let config = Config::new(limits, settings, vitri, mode, input_vtree);
    Ok(Parsed { formula, meta, record, config, program_deadline })
}

/// The ordinary non-counting route: vitri's `compile` preprocessing, the
/// paired vtree construction, then the compile of what it returned
/// ([`compile_attempt`]). Preprocessing, vtree and compile all stop at the
/// run's wall.
fn run_function_compile(run: &Run<'_>, formula: &CnfFormula) -> Result<CompileAnswer, Error> {
    let attempt = crate::frontend::compile_frontend(formula, run.config, run.deadline)
        .map_err(Error::stage_failed("compile preprocessing"))?;
    compile_attempt(run, attempt)
}

/// Reduced compilation of a compile frontend's result, or of a bundle's
/// ([`compile_with_retry`]), then exact function expansion through its total
/// original-variable map.
fn compile_attempt(run: &Run<'_>, attempt: CompileFrontendAttempt) -> Result<CompileAnswer, Error> {
    let Run { eng, config, .. } = *run;
    let CompileFrontendAttempt { reduced, expansion, telemetry, vtree } = attempt;
    let preprocessing = PreprocessReport::of(&telemetry);

    let (tdd, peak, compile_ms, answered_by) = match vtree {
        crate::frontend::FrontendVtree::Built(built) => {
            let built = built.bind(eng);
            // The portfolio's winner, `None` when no portfolio ran.
            let pick = run.announce_tree(&format!("[vtree] {}", config.vtree_name()), ": ");
            eprintln!("[compile] start");
            let clock = crate::budget::CompileClock::start(&config.vtree_time);
            let (compiled, answered_by) = compile_with_retry(run, &reduced, built, pick.as_deref())?;
            let tdd = compiled.diagram;
            let compile_ms = clock.compile_ms(0);
            let tdd = crate::frontend::expand::expand_to_originals(eng, &tdd, &expansion)
                .map_err(Error::stage_failed("diagram expansion"))?;
            // The expanded diagram is the run's last and can be its largest.
            let peak = compiled.peak_pairs.max(tdd.pair_count());
            (tdd, peak, compile_ms, answered_by)
        }
        crate::frontend::FrontendVtree::Refuted => {
            // The function is constant false over the originals, of which
            // there is at least one: vitri refuses a formula with none before
            // it preprocesses. Nothing is selected and nothing is compiled:
            // the refutation is the answer, and the zero output is the same
            // shape compiling a contradiction reaches by short-circuit.
            let zero = Tdd::zero(&Arc::new(Vtree::balanced(expansion.num_vars)));
            (zero, 0, 0, AnsweredBy::Preprocessing)
        }
        crate::frontend::FrontendVtree::FullyResolved => {
            if crate::frontend::record::is_all_constants(&expansion) {
                eprintln!("[compile] preprocessing: every variable is a backbone");
            }
            let tdd = crate::frontend::expand::fully_resolved_tdd(eng, &expansion)
                .map_err(Error::stage_failed("diagram expansion of the resolved formula"))?;
            (tdd, 0, 0, AnsweredBy::Preprocessing)
        }
    };
    // Function compilation expands every original variable back into the TDD,
    // so its intrinsic model count is already in the input formula's space.
    // The readout runs under the same limits as the compile, and a refusal
    // ends the run as the compile's would.
    let count = eng.model_count(&tdd).map_err(|error| gave_up(GiveUp::from_operation(eng, error)))?;
    Ok(CompileAnswer { tdd, count, peak_pairs: peak, preprocessing, compile_ms, answered_by })
}

/// Compile `reduced` on the frontend's vtree, `built`, and when that compile
/// runs out of memory, once more on vitri's hypergraph bisection,
/// [`RETRY_VTREE_SPEC`] ([`bisect_retry_applies`] says when). `pick` is the
/// portfolio's winner, `None` when no portfolio ran.
///
/// The portfolio picks its vtree by scoring candidates, not by compiling on
/// them, and on some formulas the diagram outgrows memory on the pick while
/// the bisection, which keeps each clause's variables together, compiles in a
/// small fraction of it. The retry runs only after the pick gave up, so a
/// formula the pick compiles is never touched. Both compiles run to the run's
/// wall, and a retry that gives up is the run's give-up: the circuit is the
/// answer, so there is no cheaper stage to fall back to.
///
/// Returns the compile that answered, over `reduced`'s variables, and which
/// one it was. The caller has named the first compile's tree in the run's
/// `started_on`; the retry names its own as it starts.
fn compile_with_retry(
    run: &Run<'_>,
    reduced: &CnfFormula,
    built: VtreeBuild,
    pick: Option<&str>,
) -> Result<(Compiled, AnsweredBy), Error> {
    let give_up = match compile_on(run, reduced, built) {
        Ok(compiled) => return Ok((compiled, AnsweredBy::Compile)),
        Err(give_up) => give_up,
    };
    // Inputs that do not fit are a fault of the route, which a second vtree
    // would only hide.
    let explicit_vtree = run.config.vtree.is_some() || run.config.input_vtree.is_some();
    if matches!(give_up, GiveUp::Invalid(_)) || !bisect_retry_applies(explicit_vtree, pick, run.deadline, Instant::now())
    {
        return Err(gave_up(give_up));
    }
    give_up.report();
    // The failed compile's diagrams and vtrees are dropped; what it leaves on
    // the engine and in the allocator is given back the way the model count's
    // fallbacks do before they compile on the same engine.
    let (held_before, held_after) = crate::compile::release_failed_compile(run.eng).held_mib();
    eprintln!(
        "[compile] bisect-retry: the compile on {} ran out of memory with {}; \
         compiling once more on {RETRY_VTREE_SPEC} (mapped+retained {} → {} MiB after the purge)",
        pick.unwrap_or(run.config.vtree_spec()),
        run.deadline.map_or_else(
            || "no time limit".to_owned(),
            |deadline| format!("{}s left", deadline.saturating_duration_since(Instant::now()).as_secs()),
        ),
        held_before,
        held_after,
    );
    run.started_on.replace(Some(RETRY_VTREE_SPEC.to_owned()));
    compile_on_bisection(run, reduced).map(|compiled| (compiled, AnsweredBy::BisectRetry)).map_err(gave_up)
}

/// One compile of `reduced` to the run's wall on a [`RETRY_VTREE_SPEC`] vtree
/// built for it now.
///
/// The construction gets the share of the time left that every standalone
/// construction gets ([`crate::budget::vtree_deadline`]), which near the wall
/// is all of it. A bisection still unfinished when it passes is no vtree, and
/// the retry gives up as a compile stopped at its deadline does, with the
/// construction's reason on stderr.
fn compile_on_bisection(run: &Run<'_>, reduced: &CnfFormula) -> Result<Compiled, GiveUp> {
    let options = VtreeOptions::default()
        .with_deadline(crate::budget::vtree_deadline(run.deadline))
        .with_run_policy(run.config);
    let built = run.config.vtree_time.time(|| build_vtree_with_components(reduced, RETRY_VTREE_SPEC, &options));
    match built {
        Ok(built) => compile_on(run, reduced, built.bind(run.eng)),
        Err(error) => {
            eprintln!("[compile] bisect-retry: no tree: {error}");
            Err(GiveUp::Stopped)
        }
    }
}

/// One compile of `reduced` on `built`, its component vtrees as the plan, to
/// the run's wall. The vtrees are dropped when it returns.
fn compile_on(run: &Run<'_>, reduced: &CnfFormula, built: VtreeBuild) -> Result<Compiled, GiveUp> {
    let plan = ComponentPlan::from_components(built.components.as_deref());
    let options = CompileOptions { plan, deadline: run.deadline, ..CompileOptions::default() };
    compile_with(run.eng, reduced, &built.root, options, run.compile)
}

/// Whether a plain compile that gave up gets the retry on
/// [`RETRY_VTREE_SPEC`]: its vtree came from the default portfolio, not an
/// explicit `--vtree` or a bundle; the portfolio's winner, `pick`, is not already of the
/// retry's family; and the run has time left at `now`.
///
/// Time left is also what makes the give-up a memory one. A compile in this
/// mode stops at its deadline or for memory: an apply refused under the
/// batch limits ([`GiveUp::Refused`]), or an allocation refused in the final
/// reduction ([`GiveUp::OutOfMemory`]). So one that stopped before the
/// deadline ran out of memory, whichever it reports.
fn bisect_retry_applies(explicit_vtree: bool, pick: Option<&str>, deadline: Option<Instant>, now: Instant) -> bool {
    !explicit_vtree
        && pick.is_none_or(|pick| spec_family(pick) != spec_family(RETRY_VTREE_SPEC))
        && deadline.is_none_or(|deadline| now < deadline)
}

/// A give-up of the plain compile route, which has nothing to fall back to:
/// reported, and the end of the run.
fn gave_up(give_up: GiveUp) -> Error {
    give_up.report();
    Error::from(give_up)
}

/// The run: read `input`, resolve its mode from `mode_flag` and the header,
/// and answer that mode under `limits` and `settings`. [`crate::count`] and
/// the binary both come through here.
///
/// The run is one shared execution context: nested limit guards and all
/// component compiles use the same meters and recycled buffers. The
/// allocator's reclamation mode (`crate::mem`) is process-wide and stays eager
/// once a run switches it. The run's memory limit lowers the process's
/// address-space limit before the instance is read, so every route runs under
/// it, and puts the limit back when the run returns.
pub(crate) fn solve(
    input: Input,
    mode_flag: Option<Mode>,
    limits: &Limits,
    settings: &Settings,
) -> Result<Answer, Failure> {
    let vitri = crate::frontend::request::resolve_env_config()
        .map_err(|error| Failure { error, stats: None })?;
    let _address_space = crate::mem::limit_address_space(limits.mem_limit_mb)
        .map_err(|error| Failure { error, stats: None })?;
    let context = Arc::new(tididi::Context::new());
    context.run(|eng| solve_on(eng, input, mode_flag, limits, settings, vitri))
}

fn solve_on(
    eng: &Engine,
    input: Input,
    mode_flag: Option<Mode>,
    limits: &Limits,
    settings: &Settings,
    vitri: vitri::RunConfig,
) -> Result<Answer, Failure> {
    // The jemalloc-backed memory probes, held for the whole run so every
    // compile sees real address-space pressure. Library use that never calls
    // `solve` keeps the no-op default.
    let _mem_pressure = eng.limits().edit(|s| {
        s.with_memory_hooks(tididi::limits::MemoryHooks::new(
            crate::mem::preflight_alloc_decay_check,
            crate::mem::mapped_bytes,
            crate::mem::rlimit_as_bytes,
            // The request-0 heartbeat, once per top-level apply.
            || crate::mem::preflight_alloc_decay_check(0),
        ))
    });

    // Start the clock, read the instance, resolve the mode.
    let Parsed { formula, meta, record, mut config, program_deadline } =
        parse_input(input, mode_flag, limits, settings, vitri).map_err(|error| Failure { error, stats: None })?;

    // The last point before the mode dispatch, so the record opens knowing the
    // mode and the resolved vtree construction and every mode fills the same
    // one.
    let mut stats = crate::output::open_record(&config, &formula);
    // The run's SAT-pruning pass, handed to every compile when the run keeps it
    // on; the tallies it gathers close the record.
    let sat_prune = SatPrune::new(config.time_limit_ms);
    // The tree the run's main compile last started on (`Run::started_on`),
    // which closes the record of a run that gives up.
    let started_on = std::cell::RefCell::new(None);

    // One engine scope covers the whole mode, including expansion and final
    // count readout. Child attempts may tighten it but cannot extend it. Each
    // mode hands back what it answered, and it is recorded here, once.
    let verdict = with_run_deadline(eng, program_deadline, || {
        // Measure only after the expiry check, before preprocessing changes the
        // signal. Opening the record first preserves an incomplete input record.
        config.source_structure_profile = Some(vitri::score::StructureProfile::measure(&formula));
        crate::budget::check_deadline(program_deadline)?;
        let config = &config;
        let run = &Run {
            eng,
            config,
            deadline: program_deadline,
            compile: CompileRun { sat_prune: config.sat_prune.then_some(&sat_prune), ..CompileRun::default() },
            started_on: &started_on,
        };
        let stats = &mut stats;
        if let Some(record) = &record {
            return answer_bundle(run, stats, formula, record);
        }
        Ok(match config.mode {
            Mode::Pwmc => {
                let problem = crate::problem::Problem::declared(formula, &meta);
                record_pwmc(stats, run_pwmc_count(run, problem, &meta)?)
            }
            Mode::Pmc => {
                let problem = crate::problem::Problem::declared(formula, &meta);
                record_pmc(stats, run_pmc_count(run, problem)?)
            }
            Mode::Wmc => record_wmc(stats, config, run_wmc_count(run, &formula, meta.declared_weights())?),
            // An explicit `--vtree` compiles on that vtree, without the cascade.
            Mode::Mc if config.vtree.is_some() => record_mc(stats, config, run_mc_streaming(run, &formula)?),
            Mode::Mc => record_mc(stats, config, run_mc_count(run, &formula)?),
            Mode::Compile => record_compile(stats, run_function_compile(run, &formula)?),
        })
    });
    crate::output::close_record(
        &mut stats,
        &verdict,
        config.vtree_time.total(),
        started_on.into_inner(),
        sat_prune.into_stats(),
    );
    match verdict {
        Ok(Verdict { satisfiable, count, diagram }) => {
            Ok(Answer { mode: config.mode, satisfiable, count, diagram, stats })
        }
        Err(error) => Err(Failure { error, stats: Some(Box::new(stats)) }),
    }
}

/// The run on a vitri bundle: the mode's compile of the bundle's reduced
/// formula on the bundle's vtree, which nothing preprocesses again, with the
/// record's lift carrying the answer back to the original formula. A compile
/// that gives up is followed by what follows one on an explicit `--vtree`.
fn answer_bundle(
    run: &Run<'_>,
    stats: &mut Stats,
    reduced: CnfFormula,
    record: &vitri::bundle::PreprocessRecord,
) -> Result<Verdict, Error> {
    let config = run.config;
    Ok(match config.mode {
        Mode::Compile => {
            let expansion = crate::frontend::record::original_expansion(record, reduced.num_vars());
            let vtree = config.input_vtree.clone().expect("a bundle's run carries the bundle's vtree");
            let attempt = CompileFrontendAttempt { reduced, expansion, telemetry: Default::default(), vtree };
            record_compile(stats, compile_attempt(run, attempt)?)
        }
        Mode::Mc => record_mc(stats, config, run_mc_bundle(run, &reduced, record)?),
        Mode::Wmc => record_wmc(stats, config, run_wmc_bundle(run, bundle_outcome(reduced, record)?)?),
        Mode::Pmc => record_pmc(stats, run_pmc_bundle(run, bundle_outcome(reduced, record)?)?),
        Mode::Pwmc => record_pwmc(stats, run_pwmc_bundle(run, bundle_outcome(reduced, record)?)?),
    })
}

/// The problem a bundle's record describes over its reduced formula.
fn bundle_outcome<D: Domain>(reduced: CnfFormula, record: &vitri::bundle::PreprocessRecord) -> Result<Outcome<D>, Error> {
    crate::frontend::record::outcome(reduced, record)
        .map_err(|error| Error::Input(format!("the bundle's preprocess.json does not fit reduced.cnf: {error}")))
}

/// The common dispatch and answer boundary: native stages may finish a step
/// after the deadline, but neither their answer nor their stage-error wrapper
/// may turn whole-run expiry into anything other than an incomplete run.
fn with_run_deadline<T>(
    eng: &Engine,
    deadline: Option<Instant>,
    run: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let _deadline = crate::budget::deadline_scope(eng, deadline);
    crate::budget::check_deadline(deadline)?;
    let outcome = run();
    crate::budget::check_deadline(deadline)?;
    outcome
}

#[cfg(test)]
mod tests;
