//! The benchmark harness's external checker, `nnf_validator`, on the `.tdd`
//! text this crate writes: it counts a written diagram as the diagram
//! counts, and counts differently or refuses when a pair is repointed or
//! dropped. Build it with `python3 scripts/bench/nnf_validator.py --build`.

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use tididi::Engine;
use tididi::Tdd;
use tididi::io::write_tdd;
use tididi_bonsai::compile::{CompileOptions, compile_cnf};
use tididi::vtree::Vtree;

/// The validator build the benchmark harness uses, resolved once per process:
/// every edit `some_edit_is_noticed` tries calls this, and starting `python3`
/// per call to re-read the same build receipt would dominate the test's time.
fn validator() -> &'static PathBuf {
    static VALIDATOR: OnceLock<PathBuf> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let output = Command::new("python3")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/bench/nnf_validator.py"))
            .arg("--path")
            .output()
            .expect("python3 is required to verify the validator build receipt");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    })
}

/// The validator's model count for `.tdd` text, or `None` when it refuses it.
fn validator_count(text: &str, name: &str) -> Option<String> {
    let path = common::tmp_path(&format!("{name}.tdd"));
    std::fs::write(&path, text).unwrap();
    let output = Command::new(validator()).arg("--json").arg(&path).output().expect("the validator runs");
    std::fs::remove_file(&path).unwrap();
    if !output.status.success() {
        return None;
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the validator reports JSON");
    Some(report["model_count"].as_str().expect("the report has a model count").to_owned())
}

fn text(tdd: &Tdd) -> String {
    let mut buf = Vec::new();
    write_tdd(&mut buf, tdd).unwrap();
    String::from_utf8(buf).unwrap()
}

/// The written text of a compiled 5-variable formula, and its count.
fn written_diagram() -> (String, String) {
    let formula = common::make_formula(5, vec![
        vec![1, 2], vec![-2, 3], vec![3, 4], vec![-4, 5], vec![-1, -5], vec![2, -3, 4],
    ]);
    let vtree = Arc::new(Vtree::random(5, 42));
    let tdd = compile_cnf(&Engine::new(), &formula, &vtree, CompileOptions::default()).unwrap().diagram;
    (text(&tdd), tdd.model_count().unwrap().to_string())
}

/// Whether the validator notices some edit of one `I` line of `text`, that
/// is, counts other than `count` or refuses. `edits` gives the replacement
/// lines for a line's fields: `I`, the node's vtree node and its two
/// children's, then the node's pairs, two child indices each.
fn some_edit_is_noticed(text: &str, count: &str, name: &str, edits: impl Fn(&[&str]) -> Vec<String>) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().enumerate().filter(|(_, line)| line.starts_with("I ")).any(|(i, line)| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        edits(&fields).iter().enumerate().any(|(k, new_line)| {
            let mut edited = lines.clone();
            edited[i] = new_line.as_str();
            validator_count(&(edited.join("\n") + "\n"), &format!("{name}_{i}_{k}")).as_deref() != Some(count)
        })
    })
}

#[test]
fn the_validator_counts_a_written_diagram() {
    let (text, count) = written_diagram();
    assert_eq!(validator_count(&text, "written"), Some(count));
}

#[test]
fn the_validator_notices_a_repointed_pair() {
    let (text, count) = written_diagram();
    let repointed = |fields: &[&str]| -> Vec<String> {
        if fields.len() < 6 {
            return Vec::new();
        }
        (0..3u32)
            .map(|v| v.to_string())
            .filter(|v| fields[4] != v.as_str())
            .map(|v| {
                let mut edited = fields.to_vec();
                edited[4] = &v;
                edited.join(" ")
            })
            .collect()
    };
    assert!(some_edit_is_noticed(&text, &count, "repointed", repointed));
}

/// Dropping one pair of a node that has several.
#[test]
fn the_validator_notices_a_dropped_pair() {
    let (text, count) = written_diagram();
    let dropped = |fields: &[&str]| -> Vec<String> {
        if fields.len() < 8 { Vec::new() } else { vec![fields[..fields.len() - 2].join(" ")] }
    };
    assert!(some_edit_is_noticed(&text, &count, "dropped", dropped));
}
