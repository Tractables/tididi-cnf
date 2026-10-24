use super::*;
use std::path::PathBuf;

/// The bundle vitri writes for `dimacs` under the request `request`, in a
/// directory of this test process named `name`.
fn bundle_of(dimacs: &str, request: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tididi_unit_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let request = vitri::request::Request::from_json(request).unwrap();
    vitri::request::prepare(dimacs.as_bytes(), &request).unwrap().write_to_dir(&dir).unwrap();
    dir
}

/// Two components, `{1, 2}` and `{3, 4}`, kept as they are.
const TWO_COMPONENTS: &str = "p cnf 4 2\n1 2 0\n3 -4 0\n";
const AS_IS: &str = r#"{"mode": "mc", "simplify": false, "arjun": false}"#;

fn input_error(result: Result<Bundle, Error>) -> String {
    match result {
        Err(Error::Input(message)) => message,
        Err(other) => panic!("expected an input error, got {other:?}"),
        Ok(_) => panic!("expected an input error, got a bundle"),
    }
}

#[test]
fn a_bundle_reads_back_with_a_tree_per_component() {
    let dir = bundle_of(TWO_COMPONENTS, AS_IS, "components");
    let bundle = read_bundle(&dir).unwrap();
    assert_eq!(bundle.reduced.num_vars(), 4);
    assert_eq!(bundle.record.mode, vitri::cnf::Mode::Mc);
    let FrontendVtree::Built(built) = bundle.vtree else { panic!("an unsettled bundle has a vtree") };
    let components = built.components.expect("two components, each with its tree");
    let mut vars: Vec<Vec<u32>> =
        components.iter().map(|c| c.component.local_to_global.iter().map(|v| v.0).collect()).collect();
    vars.sort();
    assert_eq!(vars, [[1, 2], [3, 4]]);
    assert!(components.iter().all(|c| c.component.clause_indices.len() == 1));
}

#[test]
fn a_settled_bundle_has_no_tree_to_read() {
    let unsat = bundle_of("p cnf 1 2\n1 0\n-1 0\n", r#"{"mode": "mc"}"#, "unsat");
    assert!(matches!(read_bundle(&unsat).unwrap().vtree, FrontendVtree::Refuted));
    let resolved = bundle_of("p cnf 2 2\n1 0\n2 0\n", r#"{"mode": "mc"}"#, "resolved");
    assert!(matches!(read_bundle(&resolved).unwrap().vtree, FrontendVtree::FullyResolved));
}

#[test]
fn a_record_for_another_formula_is_refused() {
    let dir = bundle_of(TWO_COMPONENTS, AS_IS, "var_count");
    std::fs::write(dir.join("reduced.cnf"), "p cnf 5 2\n1 2 0\n3 -4 0\n").unwrap();
    let message = input_error(read_bundle(&dir));
    assert!(message.contains("describes 4 reduced variables"), "{message}");
}

#[test]
fn a_vtree_over_other_variables_is_refused() {
    let dir = bundle_of(TWO_COMPONENTS, AS_IS, "vtree_leaves");
    std::fs::write(dir.join("vtree.vtree"), "vtree 3\nL 0 1\nL 2 2\nI 1 0 2\n").unwrap();
    let message = input_error(read_bundle(&dir));
    assert!(message.contains("vtree.vtree does not fit reduced.cnf"), "{message}");
}

#[test]
fn a_missing_vtree_is_refused_by_name() {
    let dir = bundle_of(TWO_COMPONENTS, AS_IS, "no_vtree");
    std::fs::remove_file(dir.join("vtree.vtree")).unwrap();
    let message = input_error(read_bundle(&dir));
    assert!(message.contains("could not open") && message.contains("vtree.vtree"), "{message}");
}

#[test]
fn a_record_vitri_does_not_know_is_refused() {
    let dir = bundle_of(TWO_COMPONENTS, AS_IS, "format_tag");
    let record = std::fs::read_to_string(dir.join("preprocess.json")).unwrap();
    std::fs::write(dir.join("preprocess.json"), record.replace("vitri-preprocess-v1", "vitri-preprocess-v0")).unwrap();
    let message = input_error(read_bundle(&dir));
    assert!(message.contains("could not read") && message.contains("preprocess.json"), "{message}");
}

#[test]
fn the_mode_is_the_bundles_and_another_is_refused() {
    let dir = bundle_of(TWO_COMPONENTS, AS_IS, "mode");
    let record = read_bundle(&dir).unwrap().record;
    assert_eq!(bundle_mode(None, &record).unwrap(), Mode::Mc);
    assert_eq!(bundle_mode(Some(Mode::Mc), &record).unwrap(), Mode::Mc);
    let error = bundle_mode(Some(Mode::Wmc), &record).unwrap_err();
    assert!(matches!(&error, Error::Usage(message) if message.contains("contradicts the bundle")), "{error:?}");
}
