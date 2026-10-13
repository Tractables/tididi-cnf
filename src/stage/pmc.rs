//! The projected model count: the stages of [`run_pmc_count`]. The
//! conditioning it falls back to is in `conditioning.rs`; the weighted sibling
//! is `pwmc.rs`.
//!
//! Conditioning on the show variables runs when the first compile, cut at a
//! share of the time limit, fails, and before one last compile on the time
//! left.

use std::time::Instant;

use vitri::cnf::CnfFormula;

use super::{Run, run_isolated};
use super::conditioning::{condition_on_show, projected_compile};
use super::projected::{
    CountVtree, Stage, Step, Trace, counting_vtree, enumerate_show, projected_arjun_budget_ms, projection_step,
    run_stages, trivial_residual,
};
use super::structured_count::{ShatterCfg, detect_shatter, structured_count_with};
use crate::vtree::VtreeOptions;
use crate::enumeration::{EnumLimits, FORECAST_AT, Models, allsat_share_ms, constrained_show_count};
use crate::output::Projected;
use crate::problem::{Problem, ShowSet, Unweighted};

/// The small-show gate, computed before Arjun on the formula as it arrived: at
/// most 32 show variables appear in its clauses. Enumerating so few show
/// projections is far cheaper than a compile, and Arjun cannot shrink an
/// already-small show set, so the gate skips projection minimization and gives
/// the enumeration ~90% of the wall rather than half. The tight bound leaves
/// larger show sets on the minimize-and-compile path, where the compile
/// answers after an enumeration its model cap stopped.
fn small_show_gate(formula: &CnfFormula, show: &ShowSet) -> bool {
    const SMALL_SHOW_MAX: usize = 32;
    let cs = constrained_show_count(formula, show);
    if cs <= SMALL_SHOW_MAX {
        eprintln!(
            "[pmc] enumerate-show: {} constrained show variables ≤ {} — Arjun skipped, enumeration first",
            cs, SMALL_SHOW_MAX
        );
        true
    } else {
        false
    }
}

/// Stage: separator-aware structured projected count.
///
/// Some near-uniform instances do not compress to a single fixed vtree — the
/// separator must sit above every component, so cross-component clauses force
/// the product diagram before pruning, and it exhausts memory — yet they
/// shatter on a small show-var separator into independent components whose
/// per-component boundary count over the separator is compact. Detect on the
/// raw formula (Arjun densifies the primal graph and hides the shatter); if it
/// shatters usefully, compute the exact projected count by combining
/// per-component boundary functions over the separator, never materializing the
/// product. Sound: the separator ⊆ show, so conditioning on it partitions
/// show-space into disjoint branches. See `structured_count.rs`.
///
/// Not window-gated: structured count is exact (not a short-wall heuristic) and
/// either returns the count fast or falls through unchanged, so it cannot hurt
/// a long run — only the cheap detector runs on non-shatter instances.
fn structured_shatter(run: &Run<'_>, problem: &Problem<Unweighted>) -> Step<Unweighted> {
    let Some(show) = problem.show.as_ref() else {
        return Step::Decline;
    };
    let formula = &problem.formula;
    let cfg = ShatterCfg {
        max_k: 24,
        min_components: 3,
        max_comp_frac: 0.5,
        max_comp_vars: 256,
    };
    let t0 = Instant::now();
    let Some(plan) = detect_shatter(formula, show, &cfg) else {
        eprintln!(
            "[pmc] shatter: no useful shatter (detect {}ms)",
            t0.elapsed().as_millis(),
        );
        return Step::Decline;
    };
    eprintln!(
        "[pmc] shatter: separator of {} variables, {} components (detect {}ms)",
        plan.separator.len(),
        plan.components.len(),
        t0.elapsed().as_millis(),
    );
    // Absolute construction budget shared by every shatter component (derived
    // once, outside the per-component closure — a fresh budget per component
    // would leave the total unbounded).
    let vtree_ctx = VtreeOptions::default()
        .with_deadline(crate::budget::vtree_deadline(run.deadline))
        .with_run_policy(run.config);
    let total = structured_count_with(run.eng, run.compile, &plan, |f| {
        // The structured counter's callback consumes one root, not a component
        // plan. Request that exact Whole construction at vitri's boundary
        // instead of building Split and discarding it.
        crate::vtree::built_or_give_up(
            run.config.vtree_time.time(|| crate::vtree::build_vtree(f, super::PORTFOLIO_VTREE_SPEC, &vtree_ctx)),
        )
    });
    match total {
        Some(total) => {
            eprintln!(
                "[pmc] shatter: counted in {}ms",
                t0.elapsed().as_millis(),
            );
            Step::Answer(total)
        }
        None => {
            eprintln!("[pmc] shatter: the batch refused a component readout — the next stage runs");
            Step::Decline
        }
    }
}

/// Stage: the Arjun-only projection-minimizing checkpoint.
///
/// This bundle is Arjun-only so the enumeration below sees exactly the
/// post-Arjun checkpoint. A preprocessing error keeps the formula as it is,
/// and so does a run with no time left for the reduction.
fn minimize_projection(run: &Run<'_>, problem: &Problem<Unweighted>) -> Step<Unweighted> {
    let Some(budget_ms) = crate::budget::clamped_to_deadline_ms(
        projected_arjun_budget_ms(run.config.time_limit_ms, problem.formula.num_vars() as u64),
        run.deadline,
        Instant::now(),
    ) else {
        return Step::Decline;
    };
    let preprocess_config = crate::frontend::request::projection_stage(
        run.config,
        vitri::cnf::Mode::Pmc,
        run.deadline,
        Some(std::time::Duration::from_millis(budget_ms)),
        vitri::config::ProjectionPolicy::ArjunOnly(vitri::config::ProjectionNoGain::Reject),
    );
    projection_step(problem, None, &preprocess_config, "[pmc] minimize-projection", "the raw formula")
}

/// Stage: small-show enumeration, then projected AllSAT.
///
/// Small-show input reaches this enum before any preprocessing; all other input
/// reaches the same fast path after its Arjun-only checkpoint.
fn enumerate(run: &Run<'_>, problem: &Problem<Unweighted>, small_show: bool) -> Step<Unweighted> {
    let Some(show) = &problem.show else {
        return Step::Decline;
    };
    // The AllSAT wall is half the run's time limit, ~90% behind the
    // small-show gate (Arjun skipped): the compile fallback cannot solve
    // these, and the count forecast applies to the widened wall alike. The
    // near-full skip routes a projection too full to enumerate to compile.
    let share = if small_show { 0.9 } else { 0.5 };
    let limits = EnumLimits {
        allsat_wall_ms: allsat_share_ms(run.config.time_limit_ms, share),
        model_budget_override: None,
        skip_near_full: true,
        deadline: run.deadline,
    };
    enumerate_show("[pmc]", &problem.formula, show, Models::default(), limits, Some(FORECAST_AT))
        .map_or(Step::Decline, Step::Answer)
}

/// Stage: the projection-preserving tail — strengthening and BVE with Arjun
/// disabled.
///
/// Enumeration declined. Resume the same vitri projection pipeline from the
/// first bundle, with Arjun off, so the tail owns strengthening and BVE without
/// rerunning or re-attributing the Arjun lift.
fn strengthen_and_eliminate(run: &Run<'_>, problem: &Problem<Unweighted>) -> Step<Unweighted> {
    let tail_config = crate::frontend::request::projection_stage(
        run.config,
        vitri::cnf::Mode::Pmc,
        run.deadline,
        None,
        vitri::config::ProjectionPolicy::Full,
    );
    projection_step(problem, None, &tail_config, "[pmc] strengthen-eliminate", "the first bundle")
}

/// Run the SAT-backed definability oracle over the residual projected vars, then
/// split the certified-defined vars into marginalize-vs-∃-forget by the
/// resolvent bound (`pos*neg > pos+neg` ⇒ declined / high-fanout) and return the
/// ∃-forget set: free vars, budget-exhausted "unknown" vars, then the defined
/// vars forgotten rather than marginalized. The bound is the first test of
/// vitri's rule for which variables its elimination tries; vitri keeps that rule
/// private, so it is written out here, without the rule's other two tests.
///
/// The oracle's budget is sound: an unknown result keeps the var, only a proven
/// UNSAT removes one — more budget trades coverage for time, never correctness.
/// The budget is the `HiddenDefinabilityConfig` default, its wall cut at
/// `deadline`, the run's wall. Once that has passed the oracle does not run,
/// and every projected var is ∃-forgotten, as an unknown one is.
///
/// The certified-defined vars are split into marginalize (factor 1) vs ∃-forget.
/// Both operations are sound for a show-defined var (Prop 1 of Lagniez, Lonca
/// and Marquis, IJCAI 2016: ∃ ≡ Σ); the split is purely about the diagram peak.
/// Marginalize only the high-fanout ("declined") defined vars, whose ∃-forget
/// OR-peak is the one worth dodging, and ∃-forget the bounded low-fanout ones
/// early — a low-fanout-defined instance is cheaper for forgetting early.
fn classify_and_split_defined(
    work: &CnfFormula,
    show: &ShowSet,
    projected_in_vtree: &[tididi::vtree::VarId],
    deadline: Option<Instant>,
) -> Vec<tididi::vtree::VarId> {
    use vitri::cnf::VarId;
    let mut definability = vitri::projection::HiddenDefinabilityConfig::default();
    if let Some(left_ms) = crate::budget::left_ms(deadline) {
        if left_ms == 0 {
            return projected_in_vtree.to_vec();
        }
        definability.time_budget =
            definability.time_budget.map(|budget| budget.min(std::time::Duration::from_millis(left_ms)));
    }
    let cls = vitri::projection::classify_hidden_defined_by_show(
        work,
        show,
        projected_in_vtree
            .iter()
            .copied()
            .map(crate::convert::formula_var),
        definability,
    )
    .expect("the shown and hidden variables are disjoint inside this formula");
    let forget_defined: Vec<VarId> = {
        // pos/neg clause-literal frequency per var, over `work`.
        let nv = work.num_vars() as usize;
        let mut freq = vec![0u64; nv * 2];
        for cl in work.clauses() {
            for lit in &cl.literals {
                freq[lit.var.idx() * 2 + if lit.positive { 0 } else { 1 }] += 1;
            }
        }
        let declined = |v: VarId| -> bool {
            let vi = v.idx();
            let (p, n) = (freq[vi * 2], freq[vi * 2 + 1]);
            p * n > p + n
        };
        // marg half = the declined vars; forget half = the rest.
        cls.defined
            .iter()
            .copied()
            .filter(|&v| !declined(v))
            .collect()
    };
    cls.not_defined
        .iter()
        .chain(cls.unknown.iter())
        .chain(forget_defined.iter())
        .copied()
        .map(crate::convert::diagram_var)
        .collect()
}

/// Stage: build the projected vtree and compile, with the reactive recovery
/// cascade behind it.
fn compile_projected(run: &Run<'_>, problem: &Problem<Unweighted>, trace: &Trace) -> Step<Unweighted> {
    let program_deadline = run.deadline;
    let work = &problem.formula;

    let vtree_started = Instant::now();
    let Some(CountVtree { vtree, component_vtrees, projected: projected_in_vtree }) =
        counting_vtree(run, work, problem.show.as_ref())
    else {
        return Step::Decline;
    };
    let vtree_ms = vtree_started.elapsed().as_millis();

    // The compile ∃-forgets each projected var as its vtree level is summed out, so
    // the full Boolean diagram never materializes.
    //
    // The projected vars to ∃-forget; the high-fanout show-defined ones are
    // marginalized instead (`classify_and_split_defined`).
    let forget_set = match &problem.show {
        Some(show) if !projected_in_vtree.is_empty() => {
            classify_and_split_defined(work, show, &projected_in_vtree, program_deadline)
        }
        _ => Vec::new(),
    };

    let t_compile = Instant::now();
    let clock = crate::budget::CompileClock::start(&run.config.vtree_time);
    // Reactive recovery: a short single-shot compile, then show-var cutset
    // conditioning, then the same compile again on the run's wall.
    //
    // The single-shot deadline is min(0.2 × wall, 2 s) from compile start. The
    // compile arms it on the engine, which cuts any apply at it, and checks it
    // between steps, so a compile not done by then gives up and conditioning
    // runs. The 2 s cap is an empirical setting: nothing in the compile
    // derives it. With no wall set there is no deadline, and only a panic or a
    // refused apply leads to recovery.
    let first_try_deadline = run.config.time_limit_ms.map(|ms| {
        let frac_ms = (ms as f64 * 0.2) as u64;
        crate::budget::not_past(t_compile + std::time::Duration::from_millis(frac_ms.min(2_000)), program_deadline)
    });
    let compile = |deadline| {
        projected_compile(
            run,
            work,
            &vtree,
            crate::compile::ComponentPlan::from_components(component_vtrees.as_deref()),
            &forget_set,
            deadline,
        )
    };
    let pmc = match run_isolated("[pmc] compile", || compile(first_try_deadline)) {
        Some(pmc) => pmc,
        None => {
            // Reclaim the failed compile's retained extents so recovery
            // starts from a clean allocator.
            let _ = crate::mem::purge_all_arenas();
            eprintln!("[pmc] compile: gave up (memory or deadline) — condition next");
            // Show vars are counted, so Σ_X ∃_Y F = Σ over show-cutset
            // assignments a of (Σ_X ∃_Y F | a). The branches account for the
            // projection and the free show vars: this is the final count.
            if let Some(pmc) = problem.show.as_ref().and_then(|show| condition_on_show(run, work, show)) {
                return Step::Answer(pmc);
            }
            eprintln!("[pmc] condition: gave up — one last compile on the time left");
            // On the run's wall: this is the last thing the instance does, and
            // with no wall its applies would poll nothing and run until the
            // external timeout kills the process.
            let Some(pmc) = compile(run.deadline) else {
                eprintln!("[pmc] no answer: the last compile gave up");
                return Step::Decline;
            };
            pmc
        }
    };
    let compile_ms = clock.compile_ms(0);
    trace.print_compile("[pmc]", work, vtree_ms, compile_ms, forget_set.len(), problem.show_len());
    Step::Answer(pmc)
}

/// Projected model counting (PMC). Counts the number of
/// distinct assignments to the show set that extend to a model of the formula.
///
/// The stages preserve a checkpoint between projection-minimizing
/// Arjun and vitri's strengthening/BVE tail so the exact small-show enumerator
/// can answer before more preprocessing. It then compiles the reduced formula
/// while existentially forgetting hidden variables. After a real ∃, each
/// forgotten var is genuinely free, so:
///
///   PMC = model_count(∃forgotten. f) / 2^forgotten
///
/// ([`Compiled::projected_count`](crate::compile::Compiled::projected_count)).
/// Every variable is a vtree leaf, so a show var appearing in no clause stays
/// free in the diagram and doubles the count there. vitri returns the
/// reduced show IDs and the exact power-of-two lift from eliminated variables;
/// the seam composes that lift once, at the answer.
///
/// Without a show set every variable is shown, and the count is a plain one:
/// only the trivial residual and the compile run.
pub(crate) fn run_pmc_count(
    run: &Run<'_>,
    problem: Problem<Unweighted>,
) -> Result<Projected<num_bigint::BigUint>, crate::Error> {
    let trivial = |problem: &Problem<Unweighted>, trace: &Trace| trivial_residual("[pmc] trivial", problem, trace);
    let compile = |problem: &Problem<Unweighted>, trace: &Trace| compile_projected(run, problem, trace);
    let Some(show) = &problem.show else {
        return run_stages(problem, &[&trivial, &compile], run.deadline, &run.config.vtree_time);
    };
    let small_show = small_show_gate(&problem.formula, show);
    let shatter = |problem: &Problem<Unweighted>, _: &Trace| structured_shatter(run, problem);
    let minimize = |problem: &Problem<Unweighted>, _: &Trace| {
        if small_show { Step::Decline } else { minimize_projection(run, problem) }
    };
    let enumerate = |problem: &Problem<Unweighted>, _: &Trace| enumerate(run, problem, small_show);
    let strengthen = |problem: &Problem<Unweighted>, _: &Trace| strengthen_and_eliminate(run, problem);
    let stages: [Stage<'_, Unweighted>; 7] = [&shatter, &minimize, &trivial, &enumerate, &strengthen, &trivial, &compile];
    run_stages(problem, &stages, run.deadline, &run.config.vtree_time)
}
