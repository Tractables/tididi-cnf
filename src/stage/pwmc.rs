//! The projected weighted model count.
//! The stages of [`run_pwmc_count`], the independent-component decomposition
//! two of them share (`pwmc_decompose`), and the weighted compile-evaluate
//! they end in. The integer sibling is `pmc.rs`.

use num_rational::BigRational;
use num_traits::One;
use std::time::Instant;

use vitri::cnf::CnfFormula;

use super::{Run, run_isolated};
use super::projected::{
    CountVtree, Stage, Step, Trace, enumerate_show, pmc_vtree, projection_step, run_stages, trivial_residual,
};
use crate::problem::WeightedCount;
use crate::enumeration::{EnumLimits, WeightedModels, allsat_share_ms};
use crate::output::Projected;
use crate::problem::{Domain, Problem, ShowSet, Weighted, problem_show};

/// The one Vitri configuration for top-level PWMC preprocessing.
///
/// The Arjun budget is a flat 300 s policy — PWMC deliberately does not mirror
/// PMC's size-adaptive fraction (`pmc_arjun_budget_ms`), the converge-vs-yield
/// balance differs per mode — never past what the program deadline still has
/// ([`crate::budget::clamped_to_deadline_ms`]).
///
/// The caller already owns the whole-program deadline and the flat Arjun
/// policy, so an available Arjun window is passed as `Exact` with no
/// `budget_ms` scale to derive it a second time. Once the deadline is spent,
/// Arjun is switched off and its budget returns to `Derived`; Vitri's projected
/// strengthening and BVE still run.
fn pwmc_preprocess_config(
    parent: &crate::config::Config,
    program_deadline: Option<Instant>,
    now: Instant,
) -> vitri::RunConfig {
    let exact_ms = crate::budget::clamped_to_deadline_ms(300_000, program_deadline, now);
    crate::frontend::request::projection_stage(
        parent,
        vitri::cnf::Mode::Pwmc,
        program_deadline,
        exact_ms.map(std::time::Duration::from_millis),
        vitri::config::ProjectionPolicy::Full,
    )
}

/// The weighted projected instance a stage is asked about: the show set and the
/// per-literal weights.
#[derive(Clone, Copy)]
struct PwmcCase<'a> {
    /// Show (Y) variables; `None` when every variable is shown. An empty set
    /// is a pure feasibility factor.
    show: Option<&'a ShowSet>,
    /// The `(negative, positive)` literal weights per variable.
    weights: &'a [(BigRational, BigRational)],
}

/// The weighted show-set enumeration: the sweep and AllSAT add up the rational
/// weight of each feasible projection. Declines on symmetric weights and when
/// there is no show set.
///
/// `allsat_wall_ms` replaces the AllSAT wall of half the run's time limit,
/// and `model_budget_override` the model cap. The component decomposition passes a tight wall, so a component
/// with many feasible projections does not spend its share of the run
/// enumerating before it compiles, and raises the model cap to 2^show for a
/// small-show component, so AllSAT ends by exhausting the projections.
/// Neither enumeration runs past `run`'s wall.
fn pwmc_show_enum_fastpath(
    run: &Run<'_>,
    formula: &CnfFormula,
    case: PwmcCase<'_>,
    allsat_wall_ms: Option<u64>,
    model_budget_override: Option<u64>,
) -> Option<WeightedCount<BigRational>> {
    let PwmcCase { show, weights } = case;
    // Symmetric weights make PWMC a scaled projected count, which
    // `symmetric_subsumption` streams.
    if weights.iter().all(|(negative, positive)| negative == positive) {
        return None;
    }
    // Unlike PMC's, this AllSAT has no near-full feasibility skip.
    let limits = EnumLimits {
        allsat_wall_ms: allsat_wall_ms
            .unwrap_or_else(|| allsat_share_ms(run.config.time_limit_ms, 0.5)),
        model_budget_override,
        skip_near_full: false,
        deadline: run.deadline,
    };
    enumerate_show("[pwmc]", formula, show?, WeightedModels::new(weights), limits)
}

/// Weighted compile + evaluate — the terminal PWMC phase. Builds the
/// (projection-aware) vtree, compiles the whole formula once on the full
/// budget, folds the diagram under the rational weights, and returns the PWMC
/// value in `work`'s own space, or `None` if the compile gave up
/// (deadline/OOM). Returning the value (rather than
/// printing) lets the independent-component decomposition multiply
/// per-component results, and leaves the lift to the one seam. The verdict is
/// the diagram's: the fold cannot tell a satisfiable formula that weighs zero
/// from an unsatisfiable one.
fn pwmc_compile_evaluate(
    run: &Run<'_>,
    work: &CnfFormula,
    trace: &Trace,
    case: PwmcCase<'_>,
) -> Option<WeightedCount<BigRational>> {
    let eng = run.eng;
    let PwmcCase { show, weights } = case;
    use num_bigint::BigInt;

    // During-compile ∃-forget lever: forget the projected-out (weight-1) vars
    // during the bottom-up compile, instead of
    // compiling the full Boolean diagram over all vars and projecting post-hoc.
    // A projected instance is mostly projected-out vars, so the post-hoc path
    // materializes a diagram far wider than the show-projected result needs.
    // Pairs with the projection-aware vtree (projected vars placed deep so the
    // forget fires before the top-of-vtree show-cone combine). On for any PWMC
    // with a show set.
    let vtree_started = Instant::now();
    let CountVtree {
        vtree,
        component_vtrees,
        projected: projected_in_vtree,
    } = pmc_vtree(run, work, show)?;
    let vtree_ms = vtree_started.elapsed().as_millis();

    // Panic-isolated: a compile OOM becomes `None`, scored unsolved, instead of
    // aborting the process. This compile sets no `CompileOptions::deadline`:
    // the run's own apply-level deadline, which the driver installs around
    // the whole run, is its wall bound.
    run_isolated("[pwmc] compile", || {
        // With a show set, forget the projected-out vars during the compile,
        // so the diagram is already projected.
        let output = if show.is_some() {
            crate::compile::Output::ProjectedFunction(&projected_in_vtree)
        } else {
            crate::compile::Output::Function
        };
        let plan = crate::compile::ComponentPlan::from_components(component_vtrees.as_deref());
        let compile = crate::compile::CompileOptions { plan, output, ..crate::compile::CompileOptions::default() };
        let clock = crate::budget::CompileClock::start(&run.config.vtree_time);
        // An apply-deadline cut returns `None` here: scored unsolved.
        let compiled = crate::compile::settle(crate::compile::compile_with(eng, work, &vtree, compile, run.compile))?;
        let g = &compiled.diagram;
        let compile_ms = clock.compile_ms(0);

        // The diagram says whether `work` has a model; the fold cannot, since a
        // zero or cancelling weight makes a satisfiable formula weigh zero.
        let value = if eng.is_sat(g).ok()? {
            // Each forgotten var weighs 2 in the fold, as it doubles a count
            // (`Compiled::projected_count`): through the marginalize-of-One,
            // or through `evaluate` weighting a free leaf ×2. Without a show
            // set nothing is forgotten and the divisor is 1.
            let sr = crate::convert::rational_weight_algebra(weights);
            let value = eng.evaluate(g, &sr).ok()?;
            WeightedCount::Satisfiable(value / BigRational::from_integer(BigInt::from(1) << compiled.forgotten))
        } else {
            WeightedCount::Unsatisfiable
        };
        trace.print_compile("[pwmc]", work, vtree_ms, compile_ms, projected_in_vtree.len(), show.map_or(0, ShowSet::len));
        Some(value)
    })
}

/// Solve a single independent component's PWMC standalone: bounded show-enumeration
/// first (cheap when the component's show is small), else the full
/// compile-evaluate on the component alone, in the component's own space —
/// the seam folds the global preprocessing lift once. Returns None if neither
/// path solves the component (the caller then aborts decomposition and keeps
/// the monolith path).
fn pwmc_solve_component(
    run: &Run<'_>,
    sub: &CnfFormula,
    local_show: &ShowSet,
    weights: &[(BigRational, BigRational)],
) -> Option<WeightedCount<BigRational>> {
    // The AllSAT budget depends on the component's show count. A component
    // with 1 to 21 show vars has at most 2^show feasible projections, which
    // AllSAT can exhaust: it gets the remaining wall less a 15 s reserve for
    // the compile, and a model cap of 2^show (under the default cap it would
    // decline, and its projected-var compile then runs out of memory, as on
    // an 18-show component seen in the corpus). A larger component cannot be enumerated, so its
    // probe stops after 4 s and the compile answers it. The components come
    // cheapest first, so the wall is still close to full when the one hard
    // component is reached. An empty local show still enumerates: the
    // component is then a feasibility factor, 1 if SAT and 0 if UNSAT. Both
    // budgets, the 4 s floor included, stop at the run's wall.
    const TIGHT_ALLSAT_MS: u64 = 4000;
    let show_ct = local_show.len() as u32;
    let (allsat_ms, model_override) = if (1..=21).contains(&show_ct) {
        let remaining_ms = crate::budget::left_ms(run.deadline).unwrap_or(120_000);
        (remaining_ms.saturating_sub(15_000).max(TIGHT_ALLSAT_MS), Some(1u64 << show_ct))
    } else {
        (TIGHT_ALLSAT_MS, None)
    };
    let case = PwmcCase { show: Some(local_show), weights };
    if let Some(v) =
        pwmc_show_enum_fastpath(run, sub, case, Some(allsat_ms), model_override)
    {
        return Some(v);
    }
    // Compile-evaluate the component alone (`build_vtree` sees one connected
    // component, so no grafting / product blow-up). Projecting the weight-1
    // (non-show) vars at the CNF level via BVE first can explode the clause set
    // (resolution blowup) and turn an easy component into a compile that never
    // finishes (seen in the corpus: the unprojected component compiles cheaply
    // through the during-forget diagram projection, while its BVE-projected
    // residual balloons). So only adopt the BVE residual when it actually
    // simplifies (fewer clauses); otherwise pass the unprojected component and let
    // `pwmc_compile_evaluate`'s during-forget ∃-project the non-show vars at
    // the diagram — the proven monolithic path. An empty residual clause ⇒
    // UNSAT ⇒ 0.
    let t_bve = Instant::now();
    let reduced = vitri::projection::eliminate_hidden(sub, local_show)
        .expect("the show set names variables of the component it came from");
    let bve_ms = t_bve.elapsed().as_millis();
    if reduced.clauses().iter().any(|c| c.literals.is_empty()) {
        return Some(WeightedCount::Unsatisfiable);
    }
    let (formula, preprocess_ms) = if reduced.clauses().len() <= sub.clauses().len() {
        (&reduced, bve_ms)
    } else {
        (sub, 0)
    };
    let trace = Trace { input_vars: sub.num_vars(), input_clauses: sub.clauses().len(), preprocess_ms };
    pwmc_compile_evaluate(run, formula, &trace, case)
}

/// Independent-component decomposition for PWMC, behind [`decompose_input`] and
/// [`decompose_reduced`]. PWMC is multiplicative over variable-disjoint
/// components: PWMC(F₁∧F₂) = PWMC(F₁)·PWMC(F₂) when var(F₁)∩var(F₂)=∅. The
/// monolithic compile grafts the component vtrees into one diagram whose size
/// is the product of the component diagram sizes; solving each component alone
/// and multiplying pays their sum instead.
///
/// Runs only on two or more components of at least two variables each; a
/// one-variable component is read off its unit clauses. `policy` says which
/// components decline it and in which order they are solved. `solve` answers
/// one component in its own variable space; a `None` from it declines the whole
/// decomposition, and the caller falls through with its budget intact.
fn pwmc_decompose<F>(
    problem: &Problem<Weighted>,
    policy: DecomposePolicy,
    label: &str,
    mut solve: F,
) -> Option<WeightedCount<BigRational>>
where
    F: FnMut(&CnfFormula, &ShowSet, &[(BigRational, BigRational)]) -> Option<WeightedCount<BigRational>>,
{
    use std::collections::HashSet;
    let DecomposePolicy { show_cap, sort_cheap_first } = policy;
    let show = problem.show.as_ref()?;
    let formula = &problem.formula;
    let mask = show.mask(formula.num_vars());
    let weights = problem_weight_tables(problem);
    let one = BigRational::one();
    let groups = formula.detect_components()?;
    // (variables, show variables, clause indices) per component of two or
    // more variables.
    let mut substantial: Vec<(usize, usize, &Vec<usize>)> = Vec::new();
    let mut singletons: Vec<&Vec<usize>> = Vec::new();
    for g in &groups {
        let mut vars: HashSet<vitri::cnf::VarId> = HashSet::new();
        for &ci in g {
            for lit in &formula.clauses()[ci].literals {
                vars.insert(lit.var);
            }
        }
        if vars.len() >= 2 {
            let show_in = vars.iter().filter(|&&v| mask.is_show(v)).count();
            substantial.push((vars.len(), show_in, g));
        } else {
            singletons.push(g);
        }
    }
    if substantial.len() < 2 {
        return None;
    }
    if let Some(cap) = show_cap
        && substantial.iter().any(|&(_, show_in, _)| show_in > cap)
    {
        return None;
    }
    // An empty clause anywhere ⇒ UNSAT ⇒ PWMC 0. detect_components puts the
    // empty clauses in a group of their own, which names no variable and which
    // the singleton loop below cannot read, so answer here.
    if formula.clauses().iter().any(|c| c.literals.is_empty()) {
        return Some(WeightedCount::Unsatisfiable);
    }
    if sort_cheap_first {
        substantial.sort_by_key(|&(nvars, _, _)| nvars);
    }
    let mut product = <Weighted as Domain>::one();
    let mut covered: HashSet<vitri::cnf::VarId> = HashSet::new();
    for &(_, _, group) in &substantial {
        let (sub, local_to_global) = formula.extract_component(group);
        let local_show = problem_show(&mask.restrict(&local_to_global));
        let mut local_weights = vec![(one.clone(), one.clone()); sub.num_vars() as usize];
        for (local, &global) in local_to_global.iter().enumerate() {
            local_weights[local] = weights[global.idx()].clone();
            if mask.is_show(global) {
                covered.insert(global);
            }
        }
        let comp_val = solve(&sub, &local_show, &local_weights);
        product = product * comp_val?;
    }
    // Singleton (1-var) components: each clause is a unit forcing that var. Both
    // polarities forced ⇒ UNSAT ⇒ 0. A forced show var contributes its forced
    // weight; a forced projected var contributes only feasibility (×1). No compile.
    for &group in &singletons {
        let mut var: Option<vitri::cnf::VarId> = None;
        let (mut pos, mut neg) = (false, false);
        for &ci in group {
            for lit in &formula.clauses()[ci].literals {
                var = Some(lit.var);
                if lit.positive {
                    pos = true;
                } else {
                    neg = true;
                }
            }
        }
        let v = var?;
        if pos && neg {
            return Some(WeightedCount::Unsatisfiable);
        }
        if mask.is_show(v) {
            covered.insert(v);
            let (w_neg, w_pos) = &weights[v.idx()];
            product = product.map(|product| product * if pos { w_pos } else { w_neg });
        }
    }
    // Show vars present in no component at all are genuinely free → (w_pos + w_neg).
    for var in show.iter_vars() {
        if !covered.contains(&var) {
            let (w_neg, w_pos) = &weights[var.idx()];
            product = product.map(|product| product * (w_pos + w_neg));
        }
    }
    eprintln!(
        "{}: {} substantial components, show {}, vars {}",
        label,
        substantial.len(),
        show.len(),
        formula.num_vars(),
    );
    Some(product)
}

/// Which components [`pwmc_decompose`] takes on, and in which order.
#[derive(Clone, Copy)]
struct DecomposePolicy {
    /// Decline as soon as a component has more show vars than this.
    show_cap: Option<usize>,
    /// Solve the components by ascending variable count.
    sort_cheap_first: bool,
}

/// Before Vitri, where components are only enumerated: at most 80 show vars
/// each, in discovery order.
const INPUT_DECOMPOSE: DecomposePolicy = DecomposePolicy { show_cap: Some(80), sort_cheap_first: false };

/// After preprocessing: no show cap, cheapest first, which
/// `pwmc_solve_component`'s budget relies on.
const REDUCED_DECOMPOSE: DecomposePolicy = DecomposePolicy { show_cap: None, sort_cheap_first: true };

/// The `(negative, positive)` literal weights per variable of a weighted
/// projected problem, in the problem's own order: a show var takes its declared
/// weights (default 1 each polarity), a projected var is pinned to 1. With no
/// show set every var is a show var.
fn problem_weight_tables(
    problem: &Problem<Weighted>,
) -> Vec<(BigRational, BigRational)> {
    let n = problem.formula.num_vars() as usize;
    let one = BigRational::one();
    let mut weights = vec![(one.clone(), one); n];
    for (idx, pair) in problem.weights.iter().enumerate().take(n) {
        if problem.show.as_ref().is_none_or(|show| show.contains(vitri::cnf::VarId::from_idx(idx))) {
            weights[idx] = pair.clone();
        }
    }
    weights
}

/// Stage: independent-component decomposition on the raw formula, before Vitri.
///
/// For a formula whose monolithic compile runs out of time or memory while
/// every component is small enough to enumerate. Each component is only
/// enumerated, never compiled, and a component with more than 80 show vars
/// declines the stage: that one is left to preprocessing and the compile,
/// with the whole budget. Any miss falls through to Vitri preprocessing with the budget
/// intact.
fn decompose_input(run: &Run<'_>, problem: &Problem<Weighted>) -> Step<Weighted> {
    let decomposed = pwmc_decompose(
        problem,
        INPUT_DECOMPOSE,
        "[pwmc] decompose",
        |sub, local_show, weights| {
            let case = PwmcCase { show: Some(local_show), weights };
            pwmc_show_enum_fastpath(run, sub, case, None, None)
        },
    );
    decomposed.map_or(Step::Decline, Step::Answer)
}

/// Stage: the one projection-preserving Vitri call.
///
/// It owns Arjun minimization, projected strengthening, BVE, show renumbering,
/// equivalence weight folds and the exact rational lift. Its keep gate: retain
/// a sound result when the show set shrank, the lift is nontrivial, or the
/// variable count fell by more than 10%. There is intentionally no second
/// caller-side gate here, and an error keeps the raw problem.
///
/// `declared_weights` are the instance's own `c p weight` lines, as parsed.
/// Vitri's projected entry is told which variables the file weighs, not the
/// resolved table, so the declaration itself has to reach this call.
fn preprocess(
    run: &Run<'_>,
    problem: &Problem<Weighted>,
    declared_weights: Option<&vitri::cnf::WeightTable>,
) -> Step<Weighted> {
    let config = pwmc_preprocess_config(run.config, run.deadline, Instant::now());
    projection_step(problem, declared_weights, &config, "[pwmc] preprocessing", "the raw formula")
}

/// Stage: small-show weighted enumeration, then weighted projected AllSAT.
///
/// For a formula with few show variables among many, whose compile grows
/// past what the run can hold. Unlike PMC's, its AllSAT has no near-full
/// feasibility skip and never takes the widened wall.
fn enumerate(run: &Run<'_>, problem: &Problem<Weighted>) -> Step<Weighted> {
    let weights = problem_weight_tables(problem);
    // Symmetric weights (w_pos == w_neg for every var) collapse PWMC to a
    // scaled integer (projected) count, which the subsumption stage below
    // streams; the enumeration declines on them.
    let case = PwmcCase { show: problem.show.as_ref(), weights: &weights };
    pwmc_show_enum_fastpath(run, &problem.formula, case, None, None)
        .map_or(Step::Decline, Step::Answer)
}

/// Stage: independent-component decomposition on the reduced residual.
///
/// Projection-set minimization and the CNF-level ∃ on projected vars can split
/// a formula that was connected before preprocessing (for example into two
/// low-width components whose joint diagram is their product). Each component
/// gets a budgeted enumeration and then its own compile
/// ([`pwmc_solve_component`]), cheapest first. Any miss falls through to the
/// monolith.
fn decompose_reduced(run: &Run<'_>, problem: &Problem<Weighted>) -> Step<Weighted> {
    let decomposed = pwmc_decompose(
        problem,
        REDUCED_DECOMPOSE,
        "[pwmc] decompose-reduced",
        |sub, local_show, weights| pwmc_solve_component(run, sub, local_show, weights),
    );
    decomposed.map_or(Step::Decline, Step::Answer)
}

/// Stage: symmetric-weight subsumption by the streaming count.
///
/// When every variable's two literal weights are equal — the all-1 case where
/// PWMC degenerates to projected model counting, or any uniform "5 5"-style
/// weighting — each model contributes the same constant ∏_{show} w_pos
/// regardless of assignment, so PWMC = (∏_{show} w_pos) × PMC. Stream the
/// integer projected count, the streaming count plain `--mc` uses with the
/// hidden variables forgotten, and scale, instead of the monolithic Boolean compile +
/// rational fold, which must materialize the whole diagram that streaming
/// never builds. On a decline (deadline/OOM) the monolithic
/// stage follows. Projected vars are weight-1 (symmetric by default), so the
/// test reduces to "all show weights symmetric".
fn symmetric_subsumption(run: &Run<'_>, problem: &Problem<Weighted>, trace: &Trace) -> Step<Weighted> {
    use num_bigint::BigInt;
    let weights = problem_weight_tables(problem);
    if weights.iter().any(|(negative, positive)| negative != positive) {
        return Step::Decline;
    }
    let work = &problem.formula;
    let n = work.num_vars() as usize;
    // ∏ over show vars of the (symmetric) literal weight; the tables pin every
    // projected var to 1. A free show var (in no clause of `work`) carries its
    // w_pos here; the streaming count counts it ×2, which makes it
    // w_pos + w_neg = 2·w_pos.
    let weight_product: BigRational = weights.iter().map(|(_, positive)| positive).product();
    let forget: Vec<tididi::vtree::VarId> = (0..n)
        .map(vitri::cnf::VarId::from_idx)
        .filter(|&v| problem.show.as_ref().is_some_and(|show| !show.contains(v)))
        .map(crate::convert::diagram_var)
        .collect();
    // Streaming and the monolithic compile each answer formulas the other
    // does not. Streaming runs first, on the run's whole wall, and a `None`
    // falls through to the monolithic compile.
    let t_stream = Instant::now();
    let streamed = {
        let components = crate::vtree::split_components(work, run.config.vtree_spec());
        super::count_streaming(run, work, &components, Some(&forget))
    };
    if let Some((pmc_present, _peak)) = streamed {
        // `pmc_present` is the projected count over `work`'s full var space:
        // The forget set ∃-forgets the projected vars (free projected vars
        // → ×1) and the streaming engine already counts free show vars at ×2, so
        // it needs no free-var correction. The projected count is zero exactly
        // when `work` has no model; the weight product cannot say that.
        let value = if pmc_present == num_bigint::BigUint::ZERO {
            WeightedCount::Unsatisfiable
        } else {
            WeightedCount::Satisfiable(BigRational::from_integer(BigInt::from(pmc_present)) * &weight_product)
        };
        eprintln!(
            "[pwmc] symmetric-subsumption: counted by streaming in {}ms (∏show w={}, preprocessing {}ms, {}c→{}c)",
            t_stream.elapsed().as_millis(),
            weight_product,
            trace.preprocess_ms,
            trace.input_clauses,
            work.clauses().len(),
        );
        return Step::Answer(value);
    }
    eprintln!(
        "[pwmc] symmetric-subsumption: streaming gave up (memory or deadline) — compile next"
    );
    Step::Decline
}

/// Stage: the weighted compile and rational fold — the terminal PWMC phase.
fn compile_evaluate(run: &Run<'_>, problem: &Problem<Weighted>, trace: &Trace) -> Step<Weighted> {
    let weights = problem_weight_tables(problem);
    let case = PwmcCase { show: problem.show.as_ref(), weights: &weights };
    match pwmc_compile_evaluate(run, &problem.formula, trace, case) {
        Some(value) => Step::Answer(value),
        // Say plainly that nothing finished in the budget: "declined" would
        // conflate a stage that did not apply with a compile cut at the deadline.
        None => {
            eprintln!("[pwmc] no answer within the time limit");
            Step::Decline
        }
    }
}

/// Projected weighted model counting (PWMC). Combines
/// ∃-projection (`run_pmc_count`) with a rational semiring fold
/// (`query::evaluate`): count the weighted assignments to the show set that
/// extend to a model, weighting only show-var literals.
///
///   PWMC = evaluate(∃forgotten. f, w) / 2^forgotten
///
/// Semantics: weights live on show vars only. Per the MC 2026 format, weights on
/// projected (Y) vars are semantically ill-defined for PWMC, so they are pinned
/// to 1 here (regardless of any stray `c p weight` line). With projected vars at
/// weight 1, every step is the rational analogue of the PMC path: each forgotten
/// var, made free by the real OR-projection, contributes a clean factor 2 in the
/// rational fold — hence the exact `/2^forgotten`.
///
/// Soundness: identical to the PMC path. BVE of a projected var is clause-level
/// ∃; since projected vars are weight-1, eliminating them at the CNF level loses
/// no weight bookkeeping (the factor we would have divided out simply never
/// appears). The component-local BVE preserves ids, so the show set and the
/// weight tables stay keyed to the component formula; the top-level Vitri bridge
/// separately returns both keyed to its final reduced formula.
///
/// Without a show set every variable is shown, and the count is a plain
/// weighted one: the decompositions and the enumeration, which work on the
/// show set, do not run.
pub(crate) fn run_pwmc_count(
    run: &Run<'_>,
    problem: Problem<Weighted>,
    meta: &vitri::cnf::CnfMeta,
) -> Result<Projected<WeightedCount<BigRational>>, crate::Error> {
    // Every stage answers in exact rationals, which no rewrite approximates.
    eprintln!("[pwmc] arithmetic: exact rational");
    let preprocess = |problem: &Problem<Weighted>, _: &Trace| preprocess(run, problem, meta.declared_weights());
    let trivial = |problem: &Problem<Weighted>, trace: &Trace| trivial_residual("[pwmc] trivial", problem, trace);
    let symmetric = |problem: &Problem<Weighted>, trace: &Trace| symmetric_subsumption(run, problem, trace);
    let compile = |problem: &Problem<Weighted>, trace: &Trace| compile_evaluate(run, problem, trace);
    if problem.show.is_none() {
        return run_stages(problem, &[&preprocess, &trivial, &symmetric, &compile], run.deadline, &run.config.vtree_time);
    }
    let decompose_input = |problem: &Problem<Weighted>, _: &Trace| decompose_input(run, problem);
    let enumerate = |problem: &Problem<Weighted>, _: &Trace| enumerate(run, problem);
    let decompose_reduced = |problem: &Problem<Weighted>, _: &Trace| decompose_reduced(run, problem);
    let stages: [Stage<'_, Weighted>; 7] =
        [&decompose_input, &preprocess, &enumerate, &trivial, &decompose_reduced, &symmetric, &compile];
    // Every stage that answers answers in its own problem's space; the seam
    // folds the lift.
    run_stages(problem, &stages, run.deadline, &run.config.vtree_time)
}

#[cfg(test)]
#[path = "tests/pwmc.rs"]
mod tests;
