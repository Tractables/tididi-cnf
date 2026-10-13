//! Projected counting over components that share boundary variables.
//!
//! For each component, compile while keeping its show and boundary variables
//! structural, forget hidden variables, and marginalize the component's own
//! show variables. An evidence counter then reads the number of feasible local
//! show assignments for each boundary assignment.
//!
//! The components are independent once the boundary assignment is fixed:
//!
//! ```text
//! total = sum over boundary assignments a of product_i component_count_i(a)
//! ```
//!
//! Components with identical local formulas and variable roles share a count
//! vector. Each vector is aligned with the common boundary during combination.
//! The cost is one compile per distinct component, `2^|boundary_i|` readouts
//! from each, and `2^|S|` products over the separator `S`: the exponential in
//! `|S|` is paid in arithmetic, not in a diagram.
//! Compilation and readout use the same batch; a refusal abandons the candidate
//! so the driver can continue its projected-compilation fallback.

use tididi::Engine;
use tididi::query::{MAX_COUNT_TABLE_VARS, PinSemantics};
use std::sync::Arc;

use num_bigint::BigUint;

use vitri::cnf::CnfFormula;
use crate::compile::{CompileOptions, CompileRun, ComponentPlan, Output, compile_with, settle};
use crate::convert::diagram_var;
use crate::formula::occurrence_counts;
use crate::problem::ShowSet;
use tididi::query::Retention;
use tididi::vtree::{VarId, Vtree, VtreeIdx};

/// Compute the dense boundary count function for one component.
///
/// `formula` is the component sub-formula (its clauses only). Its variables
/// partition into `hidden` (exist-forgotten), `own_show` (marginalized — these
/// are the component's private show vars), and `boundary` (the shared separator
/// vars, kept symbolic). The returned vector has length `2^boundary.len()`;
/// entry `a` is w(a) = number of `own_show` assignments feasible under boundary
/// assignment `a`, where bit `k` of `a` is the value of `boundary[k]`.
///
/// `vtree` must cover every variable of `formula` (all of hidden/own/boundary).
/// Boundary variables must be distinct; `shatter_component` assigns each local
/// variable to exactly one role. A boundary is part of a separator, so
/// [`detect_shatter`] keeps it within [`MAX_COUNT_TABLE_VARS`].
/// Returns `None` when compilation or readout is refused by the batch limits.
fn component_boundary_function(
    eng: &Engine,
    run: CompileRun<'_>,
    formula: &CnfFormula,
    vtree: &Arc<Vtree>,
    hidden: &[VarId],
    own_show: &[VarId],
    boundary: &[VarId],
) -> Option<Vec<BigUint>> {
    // Compile: exist-forget hidden during compile, the rest Boolean.
    let compiled = settle(compile_with(
        eng,
        formula,
        vtree,
        CompileOptions {
            plan: ComponentPlan::Whole,
            output: Output::ProjectedFunction(hidden),
            ..CompileOptions::default()
        },
        run,
    ))?;
    let mut tdd = compiled.diagram;
    assert!(
        Arc::ptr_eq(tdd.vtree(), vtree),
        "Whole boundary compilation must preserve the supplied tree before readout derives indices from it",
    );
    // Forgotten hidden leaves were ×2-marginalized; divide them back out.
    let div = BigUint::from(2u32).pow(compiled.forgotten as u32);

    // Marginalize only the own-show leaves (sum them out), leaving the boundary
    // levels Boolean. Targets must be in bottom-up topo order.
    let own_set: std::collections::HashSet<VarId> = own_show.iter().copied().collect();
    let own_leaves: Vec<VtreeIdx> = vtree.bottomup_slice()
        .iter()
        .copied()
        .filter(|&t| {
            let node = vtree.node(t);
            node.is_leaf() && own_set.contains(&vtree.leaf_var(t))
        })
        .collect();
    eng.marginalize_levels(&mut tdd, &own_leaves).ok()?;

    // Read w(a) for every boundary assignment via the pinned counter (it decodes
    // the marginal tagging internally — the safe public route).
    // A structurally-zero component has no output count slot, which the pinned
    // counter needs; every w(a) is 0. Mirrors the `is_zero` guard
    // the BigUint entry points carry.
    if tdd.is_zero() {
        return Some(vec![BigUint::ZERO; 1usize << boundary.len()]);
    }
    // One frontier counter reads every assignment, changing only the boundary
    // pins each binary increment carries through. Evidence pins restrict the
    // boundary; marginalized local counts retain their multiplicity.
    let mut ctr =
        eng.counter_with(&tdd, Retention::Frontier, PinSemantics::Evidence).ok()?;
    let mut w = ctr.count_table(boundary).ok()?;
    for count in &mut w {
        *count /= &div;
    }
    Some(w)
}

/// One component of a shatter plan, in its own compact local variable space
/// (boundary ∪ own-show ∪ hidden, nothing else — non-participating vars left in
/// would go free and inflate the count by 2 each).
pub(super) struct ShatterComponent {
    /// Sub-formula with vars compacted to `0..k`.
    pub(super) formula: CnfFormula,
    /// Local ids of the ∃-forgotten (hidden) vars.
    pub(super) hidden_local: Vec<VarId>,
    /// Local ids of this component's private show vars (marginalized).
    pub(super) own_local: Vec<VarId>,
    /// Local ids of the separator vars present in this component, ordered to
    /// match `boundary_bit` (i.e. ascending by global separator-bit index).
    pub(super) boundary_local: Vec<VarId>,
    /// `boundary_local[j]` is separator var `separator[boundary_bit[j]]` — the
    /// global bit position in the dense separator assignment.
    pub(super) boundary_bit: Vec<usize>,
}

/// A successful shatter: a separator `S ⊆ show` whose removal splits the
/// formula into independent components. The projected count equals
/// `Σ over a∈{0,1}^|S| of Π_i w_i(a)` where `w_i` is each component's boundary
/// count function (`component_boundary_function`). Sound because `S ⊆ show`, so
/// conditioning on `S` partitions show-space into disjoint branches.
pub(super) struct ShatterPlan {
    /// Separator vars (global ids), sorted ascending = canonical S-bit order.
    pub(super) separator: Vec<VarId>,
    pub(super) components: Vec<ShatterComponent>,
}

/// Gates for `detect_shatter`. Structured count only wins when the formula
/// shatters into many uniformly small components (each compiles trivially); it
/// loses (slower / OOM) when one large residual component survives. So
/// `max_comp_vars` (absolute), not a fraction, is the discriminator (small
/// formulas defeat a pure fraction; the fraction is a secondary bound for large
/// formulas), and `min_components >= 3` excludes 2-way "peel" shatters,
/// which leave a residual about as large as the formula they came from.
pub(super) struct ShatterCfg {
    /// Largest separator tried; detection never goes past
    /// [`MAX_COUNT_TABLE_VARS`].
    pub(super) max_k: usize,
    /// Fewest components a separator must leave.
    pub(super) min_components: usize,
    /// Largest component, as a fraction of the formula's variables.
    pub(super) max_comp_frac: f64,
    /// Largest component, in variables.
    pub(super) max_comp_vars: usize,
}

/// Try to find a useful separator `S ⊆ show` (occurrence-greedy prefix) whose
/// removal shatters `formula` into `>= cfg.min_components` independent
/// components, each small enough (`<= cfg.max_comp_vars` and `<= cfg.max_comp_frac`
/// of the formula) to be tractable. Grows the separator from one variable until
/// it shatters usefully or gives up at `cfg.max_k`, or at the library's
/// [`MAX_COUNT_TABLE_VARS`] whatever `cfg.max_k` asks: each component's count
/// is read out densely as the library's count table, one entry per assignment
/// of its boundary, a part of the separator.
///
/// `formula`/`show` must be the raw pre-Arjun problem — Arjun densifies the
/// residual primal graph and would hide the shatter.
pub(super) fn detect_shatter(
    formula: &CnfFormula,
    show: &ShowSet,
    cfg: &ShatterCfg,
) -> Option<ShatterPlan> {
    let ShatterCfg { max_k, min_components, max_comp_frac, max_comp_vars } = *cfg;
    use vitri::cnf::{detect_components_in, Clause};
    use std::collections::HashSet;

    let nv = formula.num_vars() as usize;

    // Per-variable clause-occurrence count; the separator is drawn from the
    // show vars that occur.
    let occ = occurrence_counts(formula, 0);
    // Show vars by descending occurrence (tiebreak: ascending id → deterministic).
    let mut show_by_occ: Vec<VarId> = show
        .iter_vars()
        .map(diagram_var)
        .filter(|v| occ[v.idx()] > 0)
        .collect();
    show_by_occ.sort_by(|a, b| occ[b.idx()].cmp(&occ[a.idx()]).then(a.0.cmp(&b.0)));

    for k in 1..=max_k.min(MAX_COUNT_TABLE_VARS).min(show_by_occ.len()) {
        let sep: Vec<VarId> = show_by_occ[..k].to_vec();
        let sep_set: HashSet<VarId> = sep.iter().copied().collect();

        // Split clauses: S-only (all lits ∈ S) vs mixed (>= 1 non-S lit).
        // detect_components runs on the stripped mixed clauses (S removed) so
        // two vars share a component iff connected through non-S vars.
        let mut mixed_orig: Vec<usize> = Vec::new();
        let mut mixed_stripped: Vec<Clause> = Vec::new();
        let mut s_only: Vec<usize> = Vec::new();
        for (i, c) in formula.clauses().iter().enumerate() {
            let non_s: Vec<_> = c
                .literals
                .iter()
                .filter(|l| !sep_set.contains(&diagram_var(l.var)))
                .copied()
                .collect();
            if non_s.is_empty() {
                s_only.push(i);
            } else {
                mixed_orig.push(i);
                mixed_stripped.push(Clause::new(non_s));
            }
        }
        if mixed_stripped.is_empty() {
            continue;
        }
        let Some(groups) = detect_components_in(&mixed_stripped, formula.num_vars()) else {
            continue; // single component at this k → grow S
        };
        if groups.len() < min_components {
            continue;
        }

        // Build the components; reject if any mixed component is too large
        // (`ShatterCfg`).
        let max_vars = (((nv as f64) * max_comp_frac).ceil() as usize).min(max_comp_vars);
        let sep_bit: std::collections::HashMap<VarId, usize> =
            sep.iter().enumerate().map(|(b, &v)| (v, b)).collect();

        let Some(mut components) = groups
            .iter()
            .map(|g| {
                let orig: Vec<usize> = g.iter().map(|&j| mixed_orig[j]).collect();
                let component = shatter_component(formula, &orig, &sep_set, show, &sep_bit);
                ((component.formula.num_vars() as usize) <= max_vars).then_some(component)
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        // S-only clauses → one extra all-boundary indicator component.
        if !s_only.is_empty() {
            components.push(shatter_component(formula, &s_only, &sep_set, show, &sep_bit));
        }

        return Some(ShatterPlan { separator: sep, components });
    }
    None
}

/// Extract the component made of `formula`'s clauses at `clauses` and classify
/// its local vars into boundary/own/hidden, ordering the boundary by global
/// separator-bit index.
fn shatter_component(
    formula: &CnfFormula,
    clauses: &[usize],
    sep_set: &std::collections::HashSet<VarId>,
    show: &ShowSet,
    sep_bit: &std::collections::HashMap<VarId, usize>,
) -> ShatterComponent {
    let (formula, local_to_global) = formula.extract_component(clauses);
    let mut hidden_local = Vec::new();
    let mut own_local = Vec::new();
    let mut boundary: Vec<(usize, VarId)> = Vec::new(); // (global bit, local id)
    for (local, global) in local_to_global.into_iter().enumerate() {
        let g = diagram_var(global);
        let lv = VarId(local as u32 + 1);
        if sep_set.contains(&g) {
            boundary.push((sep_bit[&g], lv));
        } else if show.contains(global) {
            own_local.push(lv);
        } else {
            hidden_local.push(lv);
        }
    }
    boundary.sort_by_key(|&(b, _)| b);
    let boundary_bit: Vec<usize> = boundary.iter().map(|&(b, _)| b).collect();
    let boundary_local: Vec<VarId> = boundary.iter().map(|&(_, v)| v).collect();
    ShatterComponent {
        formula,
        hidden_local,
        own_local,
        boundary_local,
        boundary_bit,
    }
}

/// Evaluate a shatter plan to the exact projected count. `build_vtree` produces
/// a vtree for each component's local formula (small → cheap), or `None` when
/// it cannot. Combines the per-component boundary functions with broadcast
/// over the full separator, so a component touching only a subset of `S`
/// aligns by global bit. Each component compiles under `run`, what the run
/// sets for each compile. A refused component, or one with no vtree, returns
/// `None` so the caller can continue its fallback sequence.
pub(super) fn structured_count_with(
    eng: &Engine,
    run: CompileRun<'_>,
    plan: &ShatterPlan,
    build_vtree: impl Fn(&CnfFormula) -> Option<Arc<Vtree>>,
) -> Option<BigUint> {
    use std::collections::HashMap;
    let s = plan.separator.len();

    // Sound opportunistic dedup: a component's boundary function w depends only
    // on its (local clauses, hidden/own/boundary role vectors) — byte-identical
    // inputs ⇒ identical w. So memoize by that key and compute each distinct
    // component once. Isomorphic gadgets extract congruently, so N copies cost
    // one compute and N cache hits, and the total is bounded by
    // 2^|S| × distinct-components.
    // Always sound: a key miss just recomputes, never a wrong reuse. The global
    // separator alignment lives in each component's `boundary_bit` (applied in
    // the combine), not in w, so components sharing w but touching different
    // separator bits still combine correctly.
    type CompKey = (Vec<Vec<(u32, bool)>>, Vec<u32>, Vec<u32>, Vec<u32>);
    let key_of = |c: &ShatterComponent| -> CompKey {
        let clauses = c
            .formula
            .clauses()
            .iter()
            .map(|cl| cl.literals.iter().map(|l| (l.var.get(), l.positive)).collect())
            .collect();
        let ids = |vs: &[VarId]| vs.iter().map(|v| v.0).collect::<Vec<u32>>();
        (clauses, ids(&c.hidden_local), ids(&c.own_local), ids(&c.boundary_local))
    };

    let mut cache: HashMap<CompKey, usize> = HashMap::new();
    let mut distinct_w: Vec<Vec<BigUint>> = Vec::new();
    let mut per_comp: Vec<(usize, &[usize])> = Vec::with_capacity(plan.components.len());
    for c in &plan.components {
        let key = key_of(c);
        let idx = if let Some(&i) = cache.get(&key) {
            i
        } else {
            let vtree = build_vtree(&c.formula)?;
            let w = component_boundary_function(
                eng,
                run,
                &c.formula,
                &vtree,
                &c.hidden_local,
                &c.own_local,
                &c.boundary_local,
            )?;
            let i = distinct_w.len();
            distinct_w.push(w);
            cache.insert(key, i);
            i
        };
        per_comp.push((idx, c.boundary_bit.as_slice()));
    }

    let mut total = BigUint::ZERO;
    for a in 0..(1usize << s) {
        let mut prod = BigUint::from(1u32);
        for &(wi, bits) in &per_comp {
            let w = &distinct_w[wi];
            let mut la = 0usize;
            for (j, &gb) in bits.iter().enumerate() {
                if (a >> gb) & 1 == 1 {
                    la |= 1 << j;
                }
            }
            prod *= &w[la];
            if prod == BigUint::ZERO {
                break;
            }
        }
        total += prod;
    }
    Some(total)
}

#[cfg(test)]
#[path = "tests/structured_count.rs"]
mod tests;
