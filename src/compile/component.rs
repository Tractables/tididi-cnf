//! Component compilation: the per-component compile-and-graft path driven by
//! `compile/mod.rs`. The descriptors it works
//! from are [`crate::vtree::ComponentVtree`] and [`crate::vtree::Component`].
//! Variable-disjoint components are compiled separately, each by the
//! bottom-up pass in `step.rs`, and grafted into one TDD here; [`CompileJob`]
//! carries the caller's work order into that pass.
use std::collections::HashSet;

use tididi::Engine;
use vitri::cnf::CnfFormula;
use super::{Compiled, GiveUp};
use super::minimize::minimize_full;
use tididi::diagram::Tdd;
use tididi::restructure::GraftError;
use tididi::diagram::WeightStore;
use crate::vtree::{Component, ComponentVtree};
use crate::convert::rational_weight_algebra;
use crate::convert::diagram_var;
use tididi::vtree::{VarId, Vtree};

use super::step::compile_component;

// ── Per-call compile inputs ──────────────────────────────────────────────────

/// The work order one compile is started with, handed down unchanged from
/// [`crate::compile::compile_cnf`] through the component split to each
/// component's own compile: when the attempt must stop and what it produces.
/// A per-component compile gets a copy whose output is restated in that
/// component's local variable numbering.
#[derive(Clone, Copy)]
pub(crate) struct CompileJob<'a> {
    /// Wall the whole attempt stops at, `None` for unbounded.
    pub(crate) deadline: Option<std::time::Instant>,
    /// What the pass produces.
    pub(crate) output: PassOutput<'a>,
    /// What the run sets for the compile.
    pub(crate) run: super::CompileRun<'a>,
}

/// [`super::Output`] as one pass reads it: the forgotten variables as a set
/// and the weights as the store the diagram carries, both in the pass's own
/// variable numbering.
#[derive(Clone, Copy)]
pub(crate) enum PassOutput<'a> {
    /// [`super::Output::Function`].
    Function,
    /// [`super::Output::Count`], or [`super::Output::WeightedCount`] when it
    /// carries a store.
    Count(Option<&'a WeightStore>),
    /// [`super::Output::ProjectedCount`].
    ProjectedCount(&'a HashSet<VarId>),
    /// [`super::Output::ProjectedFunction`].
    ProjectedFunction(&'a HashSet<VarId>),
}

impl<'a> PassOutput<'a> {
    /// Whether the pass follows a marginalize schedule: it does for every
    /// output but the Boolean function, the projected function only to time
    /// its forgets.
    pub(crate) fn scheduled(self) -> bool {
        !matches!(self, PassOutput::Function)
    }

    /// Whether the pass sums levels out.
    pub(crate) fn marginalizes(self) -> bool {
        matches!(self, PassOutput::Count(_) | PassOutput::ProjectedCount(_))
    }

    /// The weight store the diagram carries, `None` for an unweighted pass.
    pub(crate) fn weights(self) -> Option<&'a WeightStore> {
        match self {
            PassOutput::Count(weights) => weights,
            _ => None,
        }
    }

    /// A diagram the pass creates, `born`, given an empty store of the pass's
    /// weights when it counts weights. Weight values travel with the diagram
    /// that owns them and are merged wherever two diagrams meet, so every
    /// diagram gets its store where it is created, before it can reach a
    /// marginalization; the conjunctions and merges that follow carry it.
    pub(crate) fn with_store(self, mut born: Tdd) -> Tdd {
        if let Some(weights) = self.weights() {
            born.set_weights(weights.empty_like()).expect("an empty store covers a fresh diagram's columns");
        }
        born
    }

    /// The variables the pass ∃-forgets, `None` when it projects nothing.
    /// `Some` of an empty set is still a projecting pass: its leaf levels stay
    /// Boolean.
    pub(crate) fn forget(self) -> Option<&'a HashSet<VarId>> {
        match self {
            PassOutput::ProjectedCount(forget) | PassOutput::ProjectedFunction(forget) => Some(forget),
            _ => None,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The variables of a component, in its local numbering, whose global names
/// are in `global`.
pub(crate) fn local_vars_in(local_to_global: &[VarId], global: &HashSet<VarId>) -> HashSet<VarId> {
    local_to_global
        .iter()
        .enumerate()
        .filter(|(_, g)| global.contains(*g))
        .map(|(local, _)| VarId(local as u32 + 1))
        .collect()
}

/// Free-variable list for a split: the variables in `1..=total_vars` not
/// covered by any component's `local_to_global`.
fn collect_free_vars(components: &[ComponentVtree], total_vars: u32) -> Vec<VarId> {
    let in_component = covered_vars(total_vars, components.iter().map(|c| &c.component.local_to_global));
    (1..=total_vars)
        .map(VarId)
        .filter(|v| !in_component[v.idx()])
        .collect()
}

/// The number of variables in `formula.num_vars` covered by no component's
/// `local_to_global`, other than one in `forget`. These are *free* variables —
/// declared in `num_vars` but absent from every clause of every component —
/// and each contributes a factor of 2 to the model count. With no components,
/// as for a formula with no clauses, every variable is covered by none.
///
/// The streaming count builds each `sub_formula` with
/// `num_vars == component.local_to_global.len()` (see `remap_component_clauses`), so
/// its product runs only over clause-appearing vars and drops free vars; the
/// caller folds `2^free` on top. The grafted (TDD-returning) paths handle this
/// separately via `collect_free_vars` + `graft_over`, and count a projected
/// free var into [`Compiled::forgotten`] instead of leaving it out.
pub(crate) fn streaming_free_var_count(
    formula: &CnfFormula,
    components: &[Component],
    forget: Option<&HashSet<VarId>>,
) -> usize {
    let covered = covered_vars(formula.num_vars(), components.iter().map(|component| &component.local_to_global));
    // A projected var that appears in no clause is covered by no component, so
    // it never reaches a component's ∃-forget. But ∃x.φ = φ when x∉φ, so it
    // contributes factor 1, not 2: it is left out of the free-var lift.
    (1..=formula.num_vars())
        .map(VarId)
        .filter(|v| !covered[v.idx()] && !forget.is_some_and(|forget| forget.contains(v)))
        .count()
}

/// Which of the variables `1..=num_vars` some component's local-to-global map
/// names, indexed by variable.
fn covered_vars<'a>(num_vars: u32, maps: impl Iterator<Item = &'a Vec<VarId>>) -> Vec<bool> {
    let mut covered = vec![false; num_vars as usize];
    for &v in maps.flatten() {
        covered[v.idx()] = true;
    }
    covered
}

// ── `remap_component_clauses` ─────────────────────────────────────────────────

/// Reusable buffers for the per-component streaming loop.
///
/// The loop remaps one component's clauses into a self-contained sub-formula
/// before every compile; the buffers here keep that remap from allocating a
/// fresh `Vec` per clause per component, which on a run of many small
/// components is the whole cost.
///
/// Nothing here is state: every field is fully overwritten by the next
/// [`remap_component_clauses_into`], so a `RemapScratch` carries no meaning
/// between calls and a caller may reuse or discard one freely.
pub(crate) struct RemapScratch {
    /// The remapped sub-formula. Clause `Vec`s keep their capacity across calls.
    sub: CnfFormula,
    /// `global_to_local` inverse of the component's `local_to_global`.
    global_to_local: Vec<Option<u32>>,
}

impl Default for RemapScratch {
    fn default() -> Self {
        RemapScratch {
            sub: empty_formula(),
            global_to_local: Vec::new(),
        }
    }
}

impl RemapScratch {
    /// The sub-formula the last [`remap_component_clauses_into`] built.
    pub(crate) fn sub(&self) -> &CnfFormula {
        &self.sub
    }
}

/// Remap a component's clauses from global VarIds to local VarIds (`0..K-1`),
/// producing a self-contained sub-formula that can be compiled against the
/// component's small per-component vtree.
///
/// Allocating form of [`remap_component_clauses_into`], for callers that remap
/// a bounded number of components and want an owned result.
fn remap_component_clauses(
    formula: &CnfFormula,
    clause_indices: &[usize],
    local_to_global: &[VarId],
) -> CnfFormula {
    let mut scratch = RemapScratch::default();
    remap_component_clauses_into(&mut scratch, formula, clause_indices, local_to_global);
    scratch.sub
}

/// [`remap_component_clauses`] into caller-owned buffers — the single
/// implementation of the remap; the allocating form above is this plus a
/// throwaway [`RemapScratch`].
pub(crate) fn remap_component_clauses_into(
    scratch: &mut RemapScratch,
    formula: &CnfFormula,
    clause_indices: &[usize],
    local_to_global: &[VarId],
) {
    use vitri::cnf::{Clause, Literal};
    // Invert local_to_global so we can map each global var back to its local index.
    invert_local_to_global_into(&mut scratch.global_to_local, local_to_global);
    let global_to_local = &scratch.global_to_local;

    // The formula owns its clauses behind `CnfFormula::new`, so the buffer
    // leaves the previous sub-formula (a move, no allocation), is rewritten in
    // place, and goes back through the constructor at the end. Grow-or-truncate
    // rather than reallocate: the surviving clauses keep their literal
    // capacity, which is the whole point of the scratch.
    let num_vars = local_to_global.len() as u32;
    let mut clauses = std::mem::replace(&mut scratch.sub, empty_formula()).into_clauses();
    if clauses.len() > clause_indices.len() {
        clauses.truncate(clause_indices.len());
    } else {
        clauses.resize_with(clause_indices.len(), || Clause { literals: Vec::new() });
    }
    // Literals are pushed into the retained `Vec` rather than handed to
    // `Clause::new`, whose `debug_assert!` re-checks "each variable at most
    // once". `local_to_global` is injective, so the remap is a bijection on the
    // component's variables: it can neither merge two distinct variables nor
    // split one, and the source clause already passed that check when it was
    // built. Nothing is unchecked here that was ever checked.
    for (slot, &ci) in clause_indices.iter().enumerate() {
        let lits = &mut clauses[slot].literals;
        lits.clear();
        for lit in formula.clauses()[ci].literals.iter() {
            let local = global_to_local[lit.var.idx()]
                .expect("component clause references variable not in component");
            lits.push(Literal::new(vitri::cnf::VarId::from_idx(local as usize), lit.positive));
        }
    }
    // Every local index is below `local_to_global.len()`, so the constructor's
    // range check cannot fail; it costs one more pass over the literals.
    scratch.sub = CnfFormula::new(num_vars, clauses).expect("a remapped component names only local variables");
}

/// The formula with no variables and no clauses: the scratch's resting state.
fn empty_formula() -> CnfFormula {
    CnfFormula::new(0, Vec::new()).expect("an empty formula is valid")
}

/// Invert a component's `local_to_global` map (local index → global `VarId`)
/// into `out`, a `global_var_idx → Some(local_idx)` lookup (`None` for globals
/// not in this component).
fn invert_local_to_global_into(out: &mut Vec<Option<u32>>, local_to_global: &[VarId]) {
    let max_global = local_to_global.iter().map(|v| v.0).max().unwrap_or(0) as usize;
    out.clear();
    out.resize(max_global + 1, None);
    for (local, &global) in local_to_global.iter().enumerate() {
        out[global.idx()] = Some(local as u32);
    }
}

// ── `check_vtree`, `check_partition` ──────────────────────────────────────────

/// Whether `vtree` places exactly `formula`'s variables, one leaf each: a
/// variable with no leaf has nowhere to put its clauses, and a leaf with no
/// variable of the formula would be counted free.
pub(crate) fn check_vtree(formula: &CnfFormula, vtree: &Vtree) -> Result<(), GiveUp> {
    let n = formula.num_vars();
    if vtree.num_leaves() == n && (1..=n).all(|v| vtree.leaf_of(VarId(v)).is_some()) {
        return Ok(());
    }
    Err(GiveUp::Invalid(format!(
        "the vtree's {} leaves are not the formula's {n} variables",
        vtree.num_leaves(),
    )))
}

/// Whether `components` partition `formula`: every clause in exactly one
/// component, every variable in at most one, and each component's clauses
/// naming only its own variables. Each component's vtree is checked against
/// its sub-formula when that compiles.
fn check_partition(formula: &CnfFormula, components: &[ComponentVtree]) -> Result<(), GiveUp> {
    let invalid = |why: String| Err(GiveUp::Invalid(format!("the split does not partition the formula: {why}")));
    // Which component each variable is in, by variable index.
    let mut owner: Vec<Option<usize>> = vec![None; formula.num_vars() as usize];
    for (k, ComponentVtree { component, .. }) in components.iter().enumerate() {
        for v in &component.local_to_global {
            match (v.0 as usize).checked_sub(1).and_then(|i| owner.get_mut(i)) {
                Some(slot @ None) => *slot = Some(k),
                Some(Some(_)) => return invalid(format!("variable {} is in two components", v.0)),
                None => return invalid(format!("variable {} is not the formula's", v.0)),
            }
        }
    }
    let mut placed = vec![false; formula.clauses().len()];
    for (k, ComponentVtree { component, .. }) in components.iter().enumerate() {
        for &ci in &component.clause_indices {
            let Some(clause) = formula.clauses().get(ci) else {
                return invalid(format!("clause {ci} is not the formula's"));
            };
            if std::mem::replace(&mut placed[ci], true) {
                return invalid(format!("clause {ci} is in two components"));
            }
            if let Some(lit) = clause.literals.iter().find(|lit| owner[lit.var.idx()] != Some(k)) {
                return invalid(format!("clause {ci} names variable {}, which its component does not", lit.var.idx() + 1));
            }
        }
    }
    match placed.iter().position(|&p| !p) {
        Some(ci) => invalid(format!("clause {ci} is in no component")),
        None => Ok(()),
    }
}

// ── `detected_split`, `compile_split` ────────────────────────────────────────

/// The split [`super::ComponentPlan::Detect`] compiles: each of the
/// `detected` clause groups with its variables in ascending order and a
/// balanced vtree over them, the default for a caller that passes one vtree
/// for the whole formula.
pub(crate) fn detected_split(eng: &Engine, formula: &CnfFormula, detected: Vec<Vec<usize>>) -> Vec<ComponentVtree> {
    detected
        .into_iter()
        .map(|clause_indices| {
            let local_to_global: Vec<VarId> = formula
                .component_vars(&clause_indices)
                .into_iter()
                .map(diagram_var)
                .collect();
            let vtree = eng.bind_vtree(Vtree::balanced(local_to_global.len() as u32));
            ComponentVtree { vtree, component: Component { clause_indices, local_to_global } }
        })
        .collect()
}

/// Compile each component on its own vtree, under the job's variable sets and
/// weights restated in its local numbering, and graft the results over the
/// free variables into one TDD on a new grafted vtree, minimized. Each
/// per-component compile uses its own `Arc<Vtree>`, so a marginal-cluster
/// rotation stays inside it, and each component's sub-formula is remapped
/// when its turn comes, so only one is alive at a time.
///
/// A component that gives up ends the compile with its give-up; an UNSAT
/// component makes the result `Tdd::zero` on the grafted vtree, and the rest
/// are not compiled.
pub(crate) fn compile_split(
    eng: &Engine,
    formula: &CnfFormula,
    components: &[ComponentVtree],
    job: CompileJob<'_>,
) -> Result<Compiled, GiveUp> {
    check_partition(formula, components)?;
    let forget = job.output.forget();
    let weights = job.output.weights();
    let free_vars = collect_free_vars(components, formula.num_vars());
    let mut comp_tdds: Vec<(Tdd, Vec<VarId>)> = Vec::with_capacity(components.len());
    let mut forgotten = 0;
    let mut peak_pairs = 0;
    for ComponentVtree { vtree: sub_vtree, component: Component { clause_indices, local_to_global: l2g } } in components {
        let sub_formula = remap_component_clauses(formula, clause_indices, l2g);
        // The pass tests membership against the local VarIds the component
        // vtree's leaves carry, and reads weights by them; untranslated, a
        // forgotten variable would be kept inside every component.
        let comp_forget;
        let comp_weights;
        let output = match job.output {
            PassOutput::Function => PassOutput::Function,
            PassOutput::Count(weights) => {
                comp_weights = weights.map(|ws| localized_store(ws, l2g));
                PassOutput::Count(comp_weights.as_ref())
            }
            PassOutput::ProjectedCount(forget) => {
                comp_forget = local_vars_in(l2g, forget);
                PassOutput::ProjectedCount(&comp_forget)
            }
            PassOutput::ProjectedFunction(forget) => {
                comp_forget = local_vars_in(l2g, forget);
                PassOutput::ProjectedFunction(&comp_forget)
            }
        };
        let comp = compile_component(eng, &sub_formula, sub_vtree, CompileJob { output, ..job })?;
        forgotten += comp.forgotten;
        peak_pairs = peak_pairs.max(comp.peak_pairs);
        let comp_tdd = comp.diagram;
        assert!(
            weights.is_none() || comp_tdd.weights().is_some(),
            "a weighted component compiles to a diagram carrying its weight store"
        );
        if !eng.is_sat(&comp_tdd).map_err(|e| GiveUp::from_operation(eng, e))? {
            // The rest need not compile. The graft answers ⊥ with an empty
            // store when any part is ⊥, on the grafted vtree, as a satisfiable
            // result would be, so the result vtree does not depend on the
            // answer. Every part is a ⊥ on the vtree its component was given,
            // so it does not depend on what the compiles did to theirs either.
            // The output is structurally 0 and `weighted_value` short-circuits
            // on it without reading a level, so the empty store is sound here.
            comp_tdds = components.iter().map(|c| (eng.zero(&c.vtree), c.component.local_to_global.clone())).collect();
            break;
        }
        comp_tdds.push((comp_tdd, l2g.clone()));
    }
    // The free variables are grafted in as unconstrained leaves, ×2 each in the
    // raw count. No component's compile sees a projected one, so it is counted
    // as forgotten here: the result's count then covers every projected ×2 on
    // every plan.
    forgotten += forget.map_or(0, |forget| free_vars.iter().filter(|v| forget.contains(v)).count());
    // Each component's values ride into the merged store the graft fills: the
    // per-component level numberings collide, so only the graft's own layout can
    // key them.
    let (mut grafted, _layout) = Tdd::graft_over(
        eng, comp_tdds, &free_vars, formula.num_vars(), weights.map(WeightStore::empty_like),
    )
    .map_err(|error| match error {
        GraftError::Operation(e) => GiveUp::from_operation(eng, e),
        structural => GiveUp::Invalid(format!("the components do not graft: {structural}")),
    })?;
    // A ⊥ has nothing to reduce.
    if !grafted.is_zero() {
        minimize_full(eng, &mut grafted, job.run.cofactor).map_err(|_| GiveUp::OutOfMemory)?;
    }
    Ok(Compiled { diagram: grafted, forgotten, peak_pairs })
}

/// `global`'s weight table restated in a component's local variable numbering.
///
/// The marginal leaf lookup indexes the algebra by the vtree leaf's own VarId,
/// which inside a component is local, so a per-component store needs a
/// local-var algebra.
fn localized_store(global: &WeightStore, local_to_global: &[VarId]) -> WeightStore {
    let global_algebra = global.algebra();
    let weights: Vec<(num_rational::BigRational, num_rational::BigRational)> = local_to_global
        .iter()
        .map(|&g| {
            (
                global_algebra.neg_weight(g).clone(),
                global_algebra.pos_weight(g).clone(),
            )
        })
        .collect();
    WeightStore::new(
        rational_weight_algebra(&weights),
        global.arithmetic(),
    )
}

#[cfg(test)]
#[path = "tests/component.rs"]
mod tests;
