//! The reader of vitri's preprocessing record: what one call reduced a
//! problem to, the Arjun reduction it kept, and, for a compile, where every
//! original variable went.
//!
//! One [`vitri::bundle::preprocess`] call produces a
//! [`PreprocessBundle`](vitri::bundle::PreprocessBundle);
//! [`preprocess_problem`] wraps it with the problem's declarations and reads
//! the record into a [`Problem`] over the count's [`Domain`]. The one check
//! vitri's record does not settle by construction — that the show set names
//! reduced variables — happens here, so no runner re-judges vitri's result or
//! picks its own numbering convention. A compile record's map back to the
//! original variables is read once, by [`original_expansion`], into the
//! library's [`VariableExpansion`], which places the compiled diagram and its
//! vtree on the original variables.

use tididi::diagram::Literal;
use tididi::restructure::VariableExpansion;
use tididi::vtree::VarId;
use vitri::bundle::PreprocessBundle;
use vitri::cnf::CnfFormula;

use crate::problem::{Domain, Problem, ShowSet, problem_show};

/// The plain-MC Arjun result the model count's stages consume. Every field
/// is in `formula`'s reduced variable space.
#[derive(Debug)]
pub(crate) struct ArjunReduction {
    pub(crate) formula: CnfFormula,
    pub(crate) lift_pow2: u32,
    pub(crate) support: ShowSet,
}

/// What one preprocessing call left of a problem: a refutation, or the
/// problem still to count.
pub(crate) enum Outcome<D: Domain> {
    Unsat,
    Reduced(Problem<D>),
}

/// What a stage ran and how long it took.
pub(crate) struct Report {
    pub(crate) stages: vitri::bundle::StageReport,
    pub(crate) telemetry: vitri::bundle::PreprocessTelemetry,
}

/// Run one stage of vitri's preprocessing on a problem and read its record
/// once, here. `config.mode` says which chain runs; the show set and weights
/// are the problem's declarations in the formula's space.
pub(crate) fn preprocess_problem<D: Domain>(
    formula: &CnfFormula,
    show: Option<&vitri::cnf::ShowSet<vitri::cnf::Original>>,
    weights: Option<&vitri::cnf::WeightTable>,
    config: &vitri::RunConfig,
) -> Result<(Outcome<D>, Report), vitri::VitriError> {
    let meta = vitri::cnf::CnfMeta::from_parts(
        formula.num_vars(),
        config.mode,
        show.cloned(),
        // vitri's chains weigh a literal no `c p weight` line names by the
        // table's own rule, so the table is handed through as is.
        weights.cloned(),
    )?;
    let PreprocessBundle {
        reduced,
        record,
        stages,
        telemetry,
        ..
    } = vitri::bundle::preprocess(formula, &meta, config)?;
    let outcome = outcome::<D>(reduced, &record)?;
    Ok((outcome, Report { stages, telemetry }))
}

/// Read one record into the problem it describes, checking that every reduced
/// show id names the reduced formula. The record is the mode's that was asked
/// for, since an explicit `RunConfig::mode` always wins, and a projected record
/// that is not UNSAT always carries its reduced show set.
pub(crate) fn outcome<D: Domain>(
    reduced: CnfFormula,
    record: &vitri::bundle::PreprocessRecord,
) -> Result<Outcome<D>, vitri::VitriError> {
    if record.unsat {
        return Ok(Outcome::Unsat);
    }
    let show = match record.show_vars_reduced_dimacs.as_ref() {
        Some(show_record) => {
            let show = problem_show(show_record);
            if let Some(var) = show.iter_vars().find(|var| var.get() > reduced.num_vars()) {
                return Err(vitri::VitriError::mismatch(format!(
                    "{} reduced show variable {} exceeds reduced variable count {}",
                    record.mode.token(),
                    var.get(),
                    reduced.num_vars(),
                )));
            }
            Some(show)
        }
        None => None,
    };
    let (weights, lift) = D::from_record(record, reduced.num_vars());
    Ok(Outcome::Reduced(Problem {
        formula: reduced,
        show,
        weights,
        lift,
    }))
}

/// Extract the one result shape this crate's plain-MC Arjun callers accept.
///
/// vitri carries a support, possibly empty, exactly when the kept plain Arjun
/// result is the formula the bundle exports, so the support's presence is what
/// marks a usable reduction: a stage that did not run, gave up or was
/// discarded, and a refuted bundle, carry none.
pub(crate) fn arjun_reduction(bundle: PreprocessBundle) -> Option<ArjunReduction> {
    let PreprocessBundle {
        reduced,
        count_lift,
        independent_support_reduced,
        ..
    } = bundle;
    Some(ArjunReduction {
        formula: reduced,
        lift_pow2: count_lift.arjun_pow2,
        support: problem_show(&independent_support_reduced?),
    })
}

/// Whether the compile result fixes every original variable to a constant.
///
/// A fully resolved result may instead leave some originals free, which is
/// not the every-variable-a-backbone case the compile route reports on
/// stderr.
pub(crate) fn is_all_constants(expansion: &VariableExpansion) -> bool {
    expansion.classes.is_empty() && expansion.free.is_empty() && expansion.constants.len() == expansion.num_vars as usize
}

/// vitri's total original-variable map for a reduced formula of
/// `reduced_num_vars` variables, stated as the library's expansion of the
/// reduced variables into the original ones.
///
/// `classes[r]` lists, in original-variable order, every original variable
/// whose value is the reduced variable `r` (a positive literal) or its
/// negation (a negative one). Forced and free originals are kept separately
/// because they are grafted above the class-expanded compiler vtree: the
/// forced ones as one cube, the free ones as the root chain. Construct this
/// once at the vitri boundary; no compiler consumer should interpret the
/// record independently.
///
/// vitri writes that map on every compile record, one entry per original
/// variable: on a reduction it is built from the simplifier's variable fates,
/// so every literal names a reduced variable and each reduced variable is the
/// value of at least its own representative, and a refuted record carries the
/// identity. So every original lands in exactly one of the classes, the
/// constants and the free variables, and no class is empty.
pub(crate) fn original_expansion(
    record: &vitri::bundle::PreprocessRecord,
    reduced_num_vars: u32,
) -> VariableExpansion {
    let original_map = record
        .original_to_reduced_dimacs
        .as_ref()
        .expect("a compile record carries its total original map");
    let mut classes = vec![Vec::new(); reduced_num_vars as usize];
    let mut constants = Vec::new();
    let mut free = Vec::new();
    for (original, target) in original_map.iter().enumerate() {
        let original = VarId(original as u32 + 1);
        match target {
            vitri::preprocess::OriginalTarget::Literal(literal) => {
                classes[(literal.unsigned_abs() - 1) as usize].push(Literal::new(original, literal > 0));
            }
            vitri::preprocess::OriginalTarget::Constant(value) => constants.push(Literal::new(original, value)),
            vitri::preprocess::OriginalTarget::Free => free.push(original),
        }
    }
    VariableExpansion { classes, constants, free, num_vars: record.original_num_vars }
}

#[cfg(test)]
#[path = "tests/record.rs"]
mod tests;
