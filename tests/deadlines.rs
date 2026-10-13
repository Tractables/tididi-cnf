//! Whole-run deadlines include shortcut answers and final count readout.

use tididi_bonsai::{Error, Limits, Mode, count, parse_dimacs};

#[test]
fn expired_runs_cannot_answer_through_shortcuts() {
    let expired = Limits { time_limit_ms: Some(0), ..Limits::default() };
    for mode in [Mode::Compile, Mode::Mc, Mode::Wmc, Mode::Pmc, Mode::Pwmc] {
        for body in ["p cnf 2 0\n", "p cnf 2 1\n0\n"] {
            let text = if matches!(mode, Mode::Pmc | Mode::Pwmc) {
                format!("c p show 1 0\n{body}")
            } else {
                body.to_owned()
            };
            let input = || parse_dimacs(text.as_bytes()).unwrap();
            let result = count(input(), Some(mode), &expired);
            assert!(matches!(result, Err(Error::Incomplete(_))), "{mode}, {text:?}: {result:?}");

            // Expiry is per run; an unbounded run on the same thread still answers.
            let answer = count(input(), Some(mode), &Limits::default()).unwrap();
            assert_eq!(answer.satisfiable(), !body.contains("\n0\n"));
        }
    }
}

#[test]
fn the_cli_records_expiry_without_a_count_or_verdict() {
    let output = std::process::Command::new("timeout")
        .args([
            "60", env!("CARGO_BIN_EXE_tididi"), "--time-limit-ms", "0",
            "--mem-limit-mb", "0", "--output-format", "mcc", "--stats", "-",
            "tests/data/pmc_tiny_show.cnf",
        ])
        .output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("s UNKNOWN"));
    let stats: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(stats["exit_reason"], "incomplete");
    assert_eq!(stats["mode"], "pmc");
    assert!(stats.get("variables").is_some());
    assert!(stats.get("model_count").is_none());
    assert!(stats.get("satisfiable").is_none());
    assert!(lines.next().is_none());
}
