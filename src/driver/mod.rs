//! One run, start to finish: read the instance, resolve the mode, and run it.
//!
//! `solve` is the run. The library's [`crate::count`] calls it with the
//! caller's mode and limits, and the command line, `cli::run_cli`, with what
//! argv set. The phases here are the run's spine — its per-run state, reading
//! the CNF and the plain compile route — and the mode dispatch that picks
//! between that route and the stages in `crate::stage`. The stage bodies are
//! not here.

use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tididi::Engine;
use tididi::Tdd;
use tididi::vtree::Vtree;
use vitri::cnf::CnfFormula;

use crate::compile::{CompileOptions, CompileRun, ComponentPlan, GiveUp, compile_with};
use crate::compile::sat_prune::SatPrune;
use crate::config::{Config, Limits, Mode, Settings};
use crate::run::{Answer, Failure, Input};
use crate::Error;
use crate::output::{
    CompileAnswer, PreprocessReport, Verdict, record_compile, record_mc, record_pmc, record_pwmc, record_wmc,
};
use crate::stage::{Run, run_mc_count, run_mc_streaming, run_pmc_count, run_pwmc_count, run_wmc_count};

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

/// Start the run's clock, read the instance, resolve the mode, and refuse a
/// setting that mode makes inert.
/// Returns `(formula, meta, config, program_deadline)`.
/// `meta` carries the header (`c t` / `c p show` / `c p weight`); it is
/// `CnfMeta::default()` for a CNF without one.
fn parse_input(
    input: Input,
    mode_flag: Option<Mode>,
    limits: &Limits,
    settings: &Settings,
    vitri: vitri::RunConfig,
) -> Result<
    (
        CnfFormula,
        vitri::cnf::CnfMeta,
        Config,
        Option<Instant>,
    ),
    Error,
> {
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

    let (formula, meta) = match input {
        Input::File(path) => read_dimacs(&path)?,
        Input::Formula(formula, meta) => (formula, meta),
    };

    // `--mode` and the instance header (`c t`, `c p show`) decide the mode;
    // this is the one place it is decided.
    let mode = Mode::resolve(mode_flag, &meta)?;
    settings.refuse_inert(mode_flag, &meta, mode)?;

    eprintln!(
        "[input] {} variables, {} clauses",
        formula.num_vars(),
        formula.clauses().len()
    );

    Ok((formula, meta, Config::new(limits, settings, vitri, mode), program_deadline))
}

/// The ordinary non-counting route: Vitri's `compile` preprocessing, the
/// paired vtree construction, reduced compilation, then exact function
/// expansion through the returned total original-variable map. Preprocessing,
/// vtree and compile all stop at the run's wall.
fn run_function_compile(run: &Run<'_>, formula: &CnfFormula) -> Result<CompileAnswer, Error> {
    let Run { eng, config, deadline: program_deadline, .. } = *run;
    let crate::frontend::CompileFrontendAttempt { reduced, expansion, telemetry, vtree } =
        crate::frontend::compile_frontend(formula, config, program_deadline)
            .map_err(Error::stage_failed("compile preprocessing"))?;
    let preprocessing = PreprocessReport::of(&telemetry);

    let (tdd, peak, compile_ms) = match vtree {
        crate::frontend::FrontendVtree::Built(built) => {
            let built = built.bind(eng);
            let compile_vtree = built.root;
            let component_vtrees = built.components;
            eprintln!("[vtree] {}", config.vtree_spec());
            eprintln!("[compile] start");
            let clock = crate::budget::CompileClock::start(&config.vtree_time);
            let plan = ComponentPlan::from_components(component_vtrees.as_deref());
            // The compile runs to the run's wall, as every counting route's
            // does; a compile stopped there, or refused memory mid-apply, is
            // the run's give-up. There is nothing to fall back to: the circuit
            // is the answer.
            let options = CompileOptions { plan, deadline: program_deadline, ..CompileOptions::default() };
            let compiled = compile_with(eng, &reduced, &compile_vtree, options, run.compile).map_err(gave_up)?;
            let tdd = compiled.diagram;
            let compile_ms = clock.compile_ms(0);
            let tdd = crate::frontend::expand::expand_to_originals(eng, &tdd, &expansion)
                .map_err(Error::stage_failed("diagram expansion"))?;
            // The expanded diagram is the run's last and can be its largest.
            let peak = compiled.peak_pairs.max(tdd.pair_count());
            (tdd, peak, compile_ms)
        }
        crate::frontend::FrontendVtree::Refuted => {
            // The function is constant false over the originals, of which
            // there is at least one: vitri refuses a formula with none before
            // it preprocesses. Nothing is selected and nothing is compiled:
            // the refutation is the answer, and the zero output is the same
            // shape compiling a contradiction reaches by short-circuit.
            (Tdd::zero(&Arc::new(Vtree::balanced(expansion.original_num_vars))), 0, 0)
        }
        crate::frontend::FrontendVtree::FullyResolved => {
            if expansion.is_all_constants() {
                eprintln!("[compile] preprocessing: every variable is a backbone");
            }
            let tdd = crate::frontend::expand::fully_resolved_tdd(eng, &expansion)
                .map_err(Error::stage_failed("diagram expansion of the resolved formula"))?;
            (tdd, 0, 0)
        }
    };
    // Function compilation expands every original variable back into the TDD,
    // so its intrinsic model count is already in the input formula's space.
    // The readout runs under the same limits as the compile, and a refusal
    // ends the run as the compile's would.
    let count = eng.model_count(&tdd).map_err(|error| gave_up(GiveUp::from_operation(eng, error)))?;
    Ok(CompileAnswer { tdd, count, peak_pairs: peak, preprocessing, compile_ms })
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
    let context = std::sync::Arc::new(tididi::Context::new());
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
    let (formula, meta, mut config, program_deadline) = parse_input(input, mode_flag, limits, settings, vitri)
        .map_err(|error| Failure { error, stats: None })?;

    // The last point before the mode dispatch, so the record opens knowing the
    // mode and the resolved vtree construction and every mode fills the same
    // one.
    let mut stats = crate::output::open_record(&config, &formula);
    // The run's SAT-prune pass, handed to every compile when the run keeps it
    // on; the tallies it gathers close the record.
    let sat_prune = SatPrune::new(config.time_limit_ms);

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
            compile: CompileRun { sat_prune: config.sat_prune.then_some(&sat_prune), cofactor: false },
        };
        let stats = &mut stats;
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
            Mode::Compile => record_compile(stats, config, run_function_compile(run, &formula)?),
        })
    });
    crate::output::close_record(&mut stats, &verdict, config.vtree_time.total(), sat_prune.into_stats());
    match verdict {
        Ok(Verdict { satisfiable, count, diagram }) => {
            Ok(Answer { mode: config.mode, satisfiable, count, diagram, stats })
        }
        Err(error) => Err(Failure { error, stats: Some(Box::new(stats)) }),
    }
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
