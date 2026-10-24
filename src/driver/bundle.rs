//! A vitri bundle as the run's input: the directory `vitri --out-dir` writes,
//! read back as the reduced formula to compile, the record that lifts its
//! count to the original formula's, and the vtree vitri built for it.
//!
//! vitri defines the files
//! ([its bundle guide](https://github.com/Tractables/vitri/blob/main/docs/bundle.md)),
//! and they are read with its own types: the record and the component
//! manifest deserialize into vitri's structs, which refuse a format tag they
//! do not know, and every vtree goes through vitri's reader of the vtree text
//! format. What the files say is checked against `reduced.cnf` here, once, so
//! a bundle whose files do not belong together is refused as input before
//! anything compiles.

use std::path::Path;
use std::sync::Arc;

use tididi::vtree::{VarId, Vtree};
use vitri::bundle::PreprocessRecord;
use vitri::bundle::components::ComponentsManifest;
use vitri::cnf::CnfFormula;

use crate::compile::{GiveUp, check_partition, check_vtree};
use crate::config::Mode;
use crate::frontend::FrontendVtree;
use crate::vtree::{Component, ComponentVtree, VtreeBuild};
use crate::Error;

/// What a bundle holds, as the run reads it.
pub(super) struct Bundle {
    /// `reduced.cnf`, the formula the run compiles, and its header lines.
    pub(super) reduced: CnfFormula,
    pub(super) meta: vitri::cnf::CnfMeta,
    /// `preprocess.json`: the mode vitri preprocessed for, and the lift.
    pub(super) record: PreprocessRecord,
    /// `vtree.vtree` with the component trees `components.json` lists, or how
    /// the record says preprocessing settled the instance, for which vitri
    /// writes no vtree.
    pub(super) vtree: FrontendVtree,
}

/// Read the bundle in `dir`.
pub(super) fn read_bundle(dir: &Path) -> Result<Bundle, Error> {
    let record: PreprocessRecord = read_json(&dir.join("preprocess.json"))?;
    let (reduced, meta) = super::read_dimacs(&dir.join("reduced.cnf"))?;
    let described = record.reduced_to_original_dimacs.len();
    if described != reduced.num_vars() as usize {
        return Err(Error::Input(format!(
            "{} describes {described} reduced variables, but {} has {}",
            dir.join("preprocess.json").display(),
            dir.join("reduced.cnf").display(),
            reduced.num_vars(),
        )));
    }
    let vtree = if record.unsat {
        FrontendVtree::Refuted
    } else if reduced.num_vars() == 0 {
        FrontendVtree::FullyResolved
    } else {
        FrontendVtree::Built(read_vtree_build(dir, &reduced)?)
    };
    Ok(Bundle { reduced, meta, record, vtree })
}

/// The mode the bundle was made for, its record's, which the mode the caller
/// asked for (`flag`) must agree with: a reduced formula preserves only what
/// its mode counts.
pub(super) fn bundle_mode(flag: Option<Mode>, record: &PreprocessRecord) -> Result<Mode, Error> {
    let mode = Mode::of_vitri(record.mode)
        .ok_or_else(|| Error::Input(format!("the bundle's mode {} is not one tididi answers", record.mode.token())))?;
    match flag {
        Some(flag) if flag != mode => Err(Error::Usage(format!(
            "mode {flag} contradicts the bundle, which vitri preprocessed for mode {mode}. Leave \
             the mode unset to answer what the bundle was made for."
        ))),
        _ => Ok(mode),
    }
}

/// The vtree over `reduced` and, when `components.json` lists two or more
/// components, the tree of each. A connected formula's manifest lists the
/// whole formula as its one component, on `vtree.vtree` itself.
fn read_vtree_build(dir: &Path, reduced: &CnfFormula) -> Result<VtreeBuild, Error> {
    let root = read_vtree(&dir.join("vtree.vtree"), reduced.num_vars())?;
    let manifest_path = dir.join("components.json");
    let manifest: ComponentsManifest = read_json(&manifest_path)?;
    if manifest.components.len() < 2 {
        return Ok(VtreeBuild { root, components: None });
    }
    // The component each reduced variable is in. `check_partition` refuses
    // a variable the formula does not have, or one in two components.
    let mut owner = vec![None; reduced.num_vars() as usize];
    let mut components = Vec::with_capacity(manifest.components.len());
    for (k, entry) in manifest.components.iter().enumerate() {
        let local_to_global: Vec<VarId> = entry.local_to_reduced_dimacs.iter().map(|&var| VarId(var)).collect();
        for var in &local_to_global {
            if let Some(slot) = (var.0 as usize).checked_sub(1).and_then(|i| owner.get_mut(i)) {
                *slot = Some(k);
            }
        }
        let vtree = read_vtree(&dir.join(&entry.vtree), local_to_global.len() as u32)?;
        components.push(ComponentVtree { vtree, component: Component { clause_indices: Vec::new(), local_to_global } });
    }
    // A clause is in its first variable's component; `check_partition`
    // refuses one whose other variables are not, and one in none.
    for (ci, clause) in reduced.clauses().iter().enumerate() {
        if let Some(k) = clause.literals.first().and_then(|lit| owner[lit.var.idx()]) {
            components[k].component.clause_indices.push(ci);
        }
    }
    check_partition(reduced, &components).map_err(|give_up| misfit(&manifest_path, give_up))?;
    Ok(VtreeBuild { root, components: Some(components) })
}

/// The vtree at `path`, which must place exactly the `num_vars` variables of
/// the formula it is for.
fn read_vtree(path: &Path, num_vars: u32) -> Result<Arc<Vtree>, Error> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).map_err(|error| Error::Input(format!("could not open {shown}: {error}")))?;
    let vtree = vitri::vtree::Vtree::from_vtree_text(&text)
        .map_err(|error| Error::Input(format!("could not parse {shown}: {error}")))?;
    let vtree = crate::convert::vtree_from_vitri(&vtree);
    check_vtree(num_vars, &vtree).map_err(|give_up| misfit(path, give_up))?;
    Ok(Arc::new(vtree))
}

/// The JSON file at `path`, as vitri's type `T` reads it.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let shown = path.display();
    let file = std::fs::File::open(path).map_err(|error| Error::Input(format!("could not open {shown}: {error}")))?;
    serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|error| Error::Input(format!("could not read {shown}: {error}")))
}

/// A refusal of the compiler's checks that the trees and the split fit the
/// formula, as the input error it is for a bundle's `path`.
fn misfit(path: &Path, give_up: GiveUp) -> Error {
    match give_up {
        GiveUp::Invalid(why) => Error::Input(format!("{} does not fit reduced.cnf: {why}", path.display())),
        other => Error::from(other),
    }
}

#[cfg(test)]
#[path = "tests/bundle.rs"]
mod tests;
