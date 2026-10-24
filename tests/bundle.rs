//! A vitri bundle as the input: in every mode the answer on the bundle vitri
//! writes for a formula is the answer on the formula, and the binary refuses
//! the settings a bundle makes inert.

mod common;

use std::path::PathBuf;
use std::process::Command;

use tididi_cnf::{Answer, Count, Limits, Mode, count};

const BINARY: &str = env!("CARGO_BIN_EXE_tididi");

/// The bundle vitri writes for the DIMACS file at `path`, relative to the
/// crate root, under the request `request`.
fn bundle(path: &str, request: &str) -> PathBuf {
    let dimacs = std::fs::read(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let name = format!("{path}_{request}").replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let dir = common::tmp_path(&name);
    let _ = std::fs::remove_dir_all(&dir);
    let request = vitri::request::Request::from_json(request).unwrap();
    vitri::request::prepare(&dimacs, &request).unwrap().write_to_dir(&dir).unwrap();
    dir
}

fn answer(input: impl Into<tididi_cnf::Input>, mode: Mode) -> Answer {
    count(input, Some(mode), &Limits::default()).unwrap()
}

/// Whether two counts are the same, a log-domain one up to rounding.
fn same(a: &Count, b: &Count) -> bool {
    match (a, b) {
        (Count::Log(_), Count::Log(_)) => (a.to_f64() - b.to_f64()).abs() <= 1e-9 * b.to_f64().abs().max(1.0),
        _ => a == b,
    }
}

/// The request for `mode` with every stage the mode lets a request switch
/// off switched off, so the bundle's formula is the input's and its compile
/// runs on the bundle's vtree; and the request with vitri's preprocessing,
/// whose record carries a lift.
fn requests(mode: Mode) -> [String; 2] {
    let off = match mode {
        Mode::Mc | Mode::Wmc => r#""simplify": false, "arjun": false"#,
        Mode::Compile => r#""simplify": false"#,
        _ => r#""arjun": false"#,
    };
    [format!(r#"{{"mode": "{mode}", {off}}}"#), format!(r#"{{"mode": "{mode}"}}"#)]
}

#[test]
fn a_bundle_answers_what_its_formula_answers() {
    for (path, mode) in [
        ("tests/data/tiny.cnf", Mode::Compile),
        ("tests/data/mcc_mc_sat.cnf", Mode::Mc),
        ("tests/data/mcc_mc_unsat.cnf", Mode::Mc),
        ("tests/data/cnfs/LGSynth89/cm152a.cnf", Mode::Mc),
        ("tests/data/mcc_wmc_negative.cnf", Mode::Wmc),
        ("tests/data/mcc_wmc_zero.cnf", Mode::Wmc),
        ("tests/data/mcc_pmc_sat.cnf", Mode::Pmc),
        ("tests/data/mcc_pmc_unsat.cnf", Mode::Pmc),
        ("tests/data/mcc_pwmc_sat.cnf", Mode::Pwmc),
        ("tests/data/mcc_pwmc_negative.cnf", Mode::Pwmc),
    ] {
        let expected = answer(path, mode);
        for request in requests(mode) {
            let dir = bundle(path, &request);
            let got = count(dir.as_path(), None, &Limits::default())
                .unwrap_or_else(|error| panic!("{path} under {request}: {error}"));
            assert!(same(got.count(), expected.count()), "{path} under {request}: {} against {}",
                got.count(), expected.count());
            assert_eq!(got.diagram().is_some(), mode == Mode::Compile, "{path} under {request}");
        }
    }
}

/// A formula in two components, each compiled on its own tree from the
/// bundle.
#[test]
fn a_bundle_split_into_components_counts_them_all() {
    let path = common::tmp_path("two_components.cnf");
    std::fs::write(&path, "p cnf 6 4\n1 2 0\n-1 3 0\n4 5 0\n-5 6 0\n").unwrap();
    let request = r#"{"mode": "mc", "simplify": false, "arjun": false}"#;
    let dir = common::tmp_path("two_components");
    let _ = std::fs::remove_dir_all(&dir);
    let request = vitri::request::Request::from_json(request).unwrap();
    vitri::request::prepare(&std::fs::read(&path).unwrap(), &request).unwrap().write_to_dir(&dir).unwrap();
    let manifest = std::fs::read_to_string(dir.join("components.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(manifest["components"].as_array().map(Vec::len), Some(2));
    assert_eq!(answer(dir.as_path(), Mode::Mc).count(), &Count::Integer(16u32.into()));
}

/// The binary on `args`: its exit code and its standard error.
fn run(args: &[&str]) -> (Option<i32>, String) {
    let output = Command::new(BINARY).args(args).output().expect("the binary runs");
    (output.status.code(), String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn the_binary_refuses_what_a_bundle_makes_inert() {
    let dir = bundle("tests/data/mcc_mc_sat.cnf", &requests(Mode::Mc)[0]);
    let dir = dir.to_str().unwrap();
    for (args, says) in [
        (vec!["--vtree", "portfolio"], "--vtree applies to a DIMACS file"),
        (vec!["--preprocess", "none"], "--preprocess applies to a DIMACS file"),
        (vec!["--mode", "wmc"], "contradicts the bundle"),
        (vec!["--save-tdd", "out.tdd"], "the bundle's preprocess.json"),
    ] {
        let (code, stderr) = run(&[&args[..], &[dir]].concat());
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(says), "{args:?}: {stderr}");
    }
}

/// A bundle's run names no construction it was asked for: the bundle brought
/// its vtree.
#[test]
fn a_bundle_run_records_no_vtree_spec() {
    let dir = bundle("tests/data/mcc_mc_sat.cnf", &requests(Mode::Mc)[0]);
    let record = common::tmp_path("bundle_record.json");
    let (code, stderr) = run(&[dir.to_str().unwrap(), "--stats", record.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{stderr}");
    let record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(record["model_count"], "22");
    assert!(record.get("vtree_spec").is_none(), "{record}");
}
