//! The binary's command line: the spellings it accepts and refuses, what it
//! prints, and its counts on small instances against known answers.

mod common;

use std::path::PathBuf;
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_tididi");
const TINY: &str = "tests/data/tiny.cnf";

/// Environment variables a run adds to this process's own.
type Env<'a> = &'a [(&'a str, &'a str)];

/// What one run of the binary left behind.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// Whether stdout has `line` as one of its lines.
    fn prints(&self, line: &str) -> bool {
        self.stdout.lines().any(|l| l == line)
    }
}

/// The binary on `args`, with `env` added to this process's environment.
fn run(args: &[&str], env: Env) -> Run {
    let output = Command::new(BINARY).args(args).envs(env.iter().copied()).output().expect("the binary runs");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// [`run`] with `--stats`, and the record it wrote: `docs/stats.md`
/// is the field table. A run that ends before its record opens writes none,
/// and the record reads as an empty object.
fn run_with_record(args: &[&str]) -> (Run, serde_json::Value) {
    run_with_record_in(args, &[])
}

/// [`run_with_record`] with `env` added to this process's environment.
fn run_with_record_in(args: &[&str], env: Env) -> (Run, serde_json::Value) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = common::tmp_path(&format!("record_{}.json", NEXT.fetch_add(1, Ordering::Relaxed)));
    let out = run(&[args, &["--stats", path.to_str().unwrap()]].concat(), env);
    let record = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let _ = std::fs::remove_file(&path);
    (out, record)
}

/// DIMACS text in a temporary file, removed on drop.
struct TempCnf(PathBuf);

impl TempCnf {
    fn new(name: &str, text: &str) -> Self {
        let path = common::tmp_path(&format!("{name}.cnf"));
        std::fs::write(&path, text).unwrap();
        TempCnf(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempCnf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ── Spellings ────────────────────────────────────────────────────────────────

/// Every accepted vtree spelling, and preprocessing off, count the tiny formula
/// the same.
#[test]
fn accepted_spellings_count_the_same() {
    for args in [
        ["--vtree", "balanced"],
        ["--vtree", "goatd-primal"],
        ["--vtree", "portfolio"],
        ["--vtree", "flowcutter-primal"],
        ["--vtree", "flowcutter-incidence"],
        ["--vtree", "flowcutter-primal:budget=1000steps,iters=10"],
        ["--preprocess", "none"],
    ] {
        let out = run(&[&args[..], &[TINY]].concat(), &[]);
        assert!(out.ok(), "{args:?}: {}", out.stderr);
        assert!(out.prints("model count: 48"), "{args:?}: {}", out.stdout);
    }
}

/// `--help` and `-h` print the usage, vtree grammar included, to stdout and
/// exit 0, whatever else the command line holds.
#[test]
fn help_prints_the_usage() {
    for args in [vec!["--help"], vec!["-h"], vec!["-h", TINY]] {
        let out = run(&args, &[]);
        assert!(out.ok(), "{args:?}: {}", out.stderr);
        assert!(
            out.stdout.starts_with("Usage:") && out.stdout.contains("Vtree specs:"),
            "{args:?}: {}",
            out.stdout
        );
        assert!(out.stderr.is_empty(), "{args:?}: {}", out.stderr);
    }
}

/// `--version` and `-V` print the binary's name and the crate version to
/// stdout and exit 0.
#[test]
fn version_prints_the_crate_version() {
    for args in [vec!["--version"], vec!["-V"], vec!["-V", TINY]] {
        let out = run(&args, &[]);
        assert!(out.ok(), "{args:?}: {}", out.stderr);
        assert_eq!(out.stdout, format!("tididi {}\n", env!("CARGO_PKG_VERSION")), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", out.stderr);
    }
}

/// A refused command line exits with its code and names what is wrong: 2 for
/// a command line the parser refuses and for a combination or environment the
/// run validates, 1 for a vtree spec or an input the run cannot use.
#[test]
fn refusals_name_what_is_wrong() {
    let literal_above_count = TempCnf::new("literal_above_count", "p cnf 2 1\n999 0\n");
    let lone_weight = TempCnf::new("lone_weight", "c t wmc\np cnf 2 1\nc p weight 1 1 0\n1 2 0\n");
    let unit = TempCnf::new("unit", "p cnf 1 1\n1 0\n");
    let weighted =
        TempCnf::new("weighted", "c t wmc\np cnf 3 2\nc p weight 1 0.7 0\nc p weight -1 0.3 0\n1 2 0\n-1 3 0\n");
    let shown = TempCnf::new("shown", "c p show 1 0\np cnf 2 1\n1 2 0\n");
    let saved = common::tmp_path("refused.tdd");
    let saved = saved.to_str().unwrap();
    let skip: Env = &[("VITRI_PORTFOLIO_SKIP", "not-a-candidate")];
    let score: Env = &[("VITRI_SCORE_AGG", "not-a-ranker-file")];
    let learnts: Env = &[("VITRI_ARJUN_EXPORT_LEARNED_CLAUSES", "1")];
    let monotone: Env = &[("VITRI_ARJUN_SKIP_MONOTONE", "1")];
    let wall: Env = &[("TIDIDI_TIME_LIMIT_MS", "12s")];
    let vitri_wall: Env = &[("VITRI_BUDGET_MS", "60000")];
    let prune_maybe: Env = &[("TIDIDI_SAT_PRUNE", "maybe")];
    let prune_off: Env = &[("TIDIDI_SAT_PRUNE", "off")];
    let prune_on: Env = &[("TIDIDI_SAT_PRUNE", "on")];

    // (exit code, arguments, environment, what stderr names)
    let rows: Vec<(i32, Vec<&str>, Env, Vec<&str>)> = vec![
        (1, vec!["--vtree", "nonexistent", TINY], &[], vec!["unknown vtree type"]),
        // A preprocessing level that does not exist names the ones that do.
        (2, vec!["--preprocess", "half", TINY], &[], vec!["half", "none, full"]),
        // What the argument parser refuses is a usage error, named on stderr.
        (2, vec![], &[], vec!["no DIMACS file", "Usage:"]),
        (2, vec!["--vtree"], &[], vec!["--vtree", "Usage:"]),
        (2, vec!["--no-such-flag", TINY], &[], vec!["--no-such-flag"]),
        (2, vec!["--time-limit-ms", "2s", TINY], &[], vec!["--time-limit-ms", "\"2s\""]),
        (2, vec!["--mem-limit-mb", "lots", TINY], &[], vec!["--mem-limit-mb", "\"lots\""]),
        (2, vec!["--mode", "count", TINY], &[], vec!["--mode", "\"count\"", "compile, mc, wmc, pmc, pwmc"]),
        // A flag is read once, whether or not its values agree.
        (2, vec!["--mode", "mc", "--mode", "wmc", TINY], &[], vec!["--mode is given twice"]),
        (2, vec!["--mode", "mc", "--mode", "mc", TINY], &[], vec!["--mode is given twice"]),
        (2, vec!["--vtree", "portfolio", "--vtree", "balanced", TINY], &[], vec!["--vtree is given twice"]),
        (2, vec!["--preprocess", "none", "--preprocess", "full", TINY], &[], vec!["--preprocess is given twice"]),
        (2, vec!["--output-format", "mcc", "--output-format", "plain", TINY], &[], vec!["--output-format is given twice"]),
        (2, vec!["--time-limit-ms", "1", "--time-limit-ms", "60000", TINY], &[], vec!["--time-limit-ms is given twice"]),
        (2, vec!["--mem-limit-mb", "0", "--mem-limit-mb", "0", TINY], &[], vec!["--mem-limit-mb is given twice"]),
        (2, vec!["--stats", "-", "--stats", "-", TINY], &[], vec!["--stats is given twice"]),
        (2, vec!["--save-tdd", saved, "--save-tdd", saved, TINY], &[], vec!["--save-tdd is given twice"]),
        (2, vec!["--output-format", "sharp", TINY], &[], vec!["--output-format", "\"sharp\"", "plain", "mcc"]),
        // A second input file is refused, not read in place of the first.
        (2, vec![TINY, "tests/data/mc_header_tiny.cnf"], &[], vec!["tests/data/mc_header_tiny.cnf", "one DIMACS file"]),
        (1, vec!["/nonexistent/tididi_test.cnf"], &[], vec!["error: could not open"]),
        (1, vec![literal_above_count.path()], &[], vec!["error: could not parse"]),
        // A literal weighed alone at a weight outside (0, 1): the competition's
        // format gives the other literal no weight, so the file is refused.
        (
            1,
            vec![lone_weight.path()],
            &[],
            vec!["error: could not parse", "weight literal 1 has weight 1 and literal -1 has none", "give both literals"],
        ),
        // Only the compile mode's diagram is the circuit whose count is the
        // answer, whether `--mode` or the header made the run count.
        (2, vec!["--mode", "mc", "--save-tdd", saved, TINY], &[], vec!["--save-tdd", "--mode mc"]),
        (2, vec!["--mode", "wmc", "--save-tdd", saved, TINY], &[], vec!["--save-tdd", "--mode wmc"]),
        (2, vec!["--mode", "pmc", "--save-tdd", saved, TINY], &[], vec!["--save-tdd", "--mode pmc"]),
        (2, vec!["--save-tdd", saved, "tests/data/mc_header_tiny.cnf"], &[], vec!["--save-tdd", "c t mc"]),
        (2, vec!["--save-tdd", saved, weighted.path()], &[], vec!["--save-tdd", "c t wmc"]),
        (2, vec!["--save-tdd", saved, shown.path()], &[], vec!["--save-tdd", "c p show"]),
        // The projected counts simplify inside their projection chain, which
        // `--preprocess` does not reach, whichever value it has.
        (2, vec!["--preprocess", "none", "--mode", "pmc", TINY], &[], vec!["--preprocess", "--mode pmc", "pmc"]),
        (2, vec!["--preprocess", "full", shown.path()], &[], vec!["--preprocess", "c p show", "pmc"]),
        (2, vec!["--preprocess", "full", "--mode", "pwmc", TINY], &[], vec!["--preprocess", "pwmc"]),
        // The weighted count has one construction, whether `--mode` or the
        // header asked for it, so `--vtree` is refused, and before
        // preprocessing, which would refute this formula and answer.
        (2, vec!["--mode", "wmc", "--vtree", "portfolio", "tests/data/wmc_unsat.cnf"], &[], vec!["--vtree", "--mode wmc"]),
        (2, vec!["--vtree", "portfolio", "tests/data/wmc_unsat.cnf"], &[], vec!["--vtree", "c t wmc"]),
        (2, vec![TINY, "--mode", "mc"], skip, vec!["VITRI_PORTFOLIO_SKIP"]),
        (2, vec![weighted.path()], skip, vec!["VITRI_PORTFOLIO_SKIP"]),
        (2, vec![unit.path(), "--mode", "mc"], score, vec!["VITRI_SCORE_AGG"]),
        // This build does not use Arjun's learnt clauses, so asking vitri to
        // export them is refused rather than ignored.
        (2, vec![TINY, "--mode", "mc"], learnts, vec!["VITRI_ARJUN_EXPORT_LEARNED_CLAUSES", "learnt clauses"]),
        // A model count asks vitri for the monotone skip itself.
        (2, vec![TINY, "--mode", "mc"], monotone, vec!["VITRI_ARJUN_SKIP_MONOTONE", "model count"]),
        // An unparsable time limit is refused, even beside the flag that
        // would win.
        (2, vec![TINY, "--time-limit-ms", "60000"], wall, vec!["TIDIDI_TIME_LIMIT_MS", "\"12s\""]),
        // The run's time limit is `--time-limit-ms`; vitri's own would be
        // overwritten.
        (2, vec![TINY, "--mode", "mc", "--time-limit-ms", "60000"], vitri_wall, vec!["VITRI_BUDGET_MS", "time limit"]),
        // The SAT-pruning switch reads `on` or `off`, and nothing else as
        // either.
        (2, vec![TINY], prune_maybe, vec!["TIDIDI_SAT_PRUNE", "\"maybe\"", "on", "off"]),
        // The weighted count's compiles never prune, so the switch is refused
        // there, set to either value, whether `--mode` or the header asked
        // for the weighted count.
        (2, vec!["--mode", "wmc", TINY], prune_off, vec!["TIDIDI_SAT_PRUNE", "--mode wmc", "compile, mc, pmc and pwmc"]),
        (2, vec![weighted.path()], prune_on, vec!["TIDIDI_SAT_PRUNE", "c t wmc"]),
    ];

    for (code, args, env, names) in rows {
        let out = run(&args, env);
        assert_eq!(out.code, Some(code), "{args:?}: {}", out.stderr);
        for name in names {
            assert!(out.stderr.contains(name), "{args:?} must name {name:?}: {}", out.stderr);
        }
    }
    assert!(!std::path::Path::new(saved).exists(), "a refused --save-tdd wrote a diagram");
}

/// A reader that stops early closes stdout under the run, which still ends
/// with its status and no panic, whatever it was printing.
#[test]
fn a_closed_stdout_ends_the_run_quietly() {
    use std::process::Stdio;

    for args in [vec![TINY], vec!["--output-format", "mcc", TINY], vec![TINY, "--stats", "-"], vec!["--help"]] {
        let mut child = Command::new(BINARY)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary runs");
        drop(child.stdout.take());
        let output = child.wait_with_output().expect("the binary ends");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    }
}

/// An argument that is not text is read, never a panic: the input path and
/// a record path are taken as the system gave them, and a flag value that
/// must be text is refused by name.
#[test]
fn arguments_need_not_be_text() {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt;

    let run_os = |args: &[&OsStr]| {
        let output = Command::new(BINARY).args(args).output().expect("the binary runs");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    let (code, stderr) = run_os(&[OsStr::from_bytes(b"/nonexistent/\xff.cnf")]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("could not open"), "{stderr}");

    let (code, stderr) = run_os(&[OsStr::new("--vtree"), OsStr::from_bytes(b"\xff"), OsStr::new(TINY)]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("--vtree") && stderr.contains("not text"), "{stderr}");

    let mut record = OsString::from(common::tmp_path("record_"));
    record.push(OsStr::from_bytes(b"\xff.json"));
    let (code, stderr) = run_os(&[OsStr::new(TINY), OsStr::new("--stats"), &record]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(std::fs::remove_file(&record).is_ok(), "the record was written at the path given");
}

/// `--mode` names the mode, and without it the header declares one: a `c t`
/// line names the count, a `c p show` line makes it projected, and a CNF that
/// declares neither is compiled. `c p weight` lines alone declare nothing, but
/// `--mode mc` and `--mode pmc`, which would ignore them, refuse them and name
/// the weighted modes; `--mode compile` reads past them. A `--mode` the header
/// contradicts is refused, naming the mode and the header lines; a lone
/// `c p show` line agrees with either projected mode.
#[test]
fn the_mode_comes_from_the_flag_or_the_header() {
    let body = "p cnf 3 2\n1 2 0\n-2 3 0\n";
    let header = |name: &str, lines: &str| TempCnf::new(&format!("mode_{name}"), &format!("{lines}{body}"));
    let plain = header("plain", "");
    let weights = header("weights", "c p weight 1 3/10 0\n");
    let show = header("show", "c p show 1 2 0\n");
    let show_weights = header("show_weights", "c p show 1 2 0\nc p weight 1 3/10 0\n");
    let t_mc = header("t_mc", "c t mc\n");
    let t_mc_weights = header("t_mc_weights", "c t mc\nc p weight 1 3/10 0\n");
    let t_mc_show = header("t_mc_show", "c t mc\nc p show 1 2 0\n");
    let t_wmc = header("t_wmc", "c t wmc\nc p weight 1 3/10 0\n");
    let t_wmc_show = header("t_wmc_show", "c t wmc\nc p show 1 2 0\nc p weight 1 3/10 0\n");
    let t_pmc = header("t_pmc", "c t pmc\nc p show 1 2 0\n");
    let t_pmc_weights = header("t_pmc_weights", "c t pmc\nc p show 1 2 0\nc p weight 1 3/10 0\n");
    let t_pwmc = header("t_pwmc", "c t pwmc\nc p show 1 2 0\nc p weight 1 3/10 0\n");

    // (--mode, instance, the resolved mode, or the refusal's names)
    type Row<'a> = (Option<&'a str>, &'a TempCnf, Result<&'a str, &'a [&'a str]>);
    let rows: [Row; 29] = [
        (None, &plain, Ok("compile")),
        (None, &weights, Ok("compile")),
        (None, &show, Ok("pmc")),
        (None, &show_weights, Ok("pmc")),
        (None, &t_mc, Ok("mc")),
        (None, &t_mc_weights, Ok("mc")),
        (None, &t_mc_show, Ok("pmc")),
        (None, &t_wmc, Ok("wmc")),
        (None, &t_wmc_show, Ok("pwmc")),
        (None, &t_pmc, Ok("pmc")),
        (None, &t_pmc_weights, Ok("pmc")),
        (None, &t_pwmc, Ok("pwmc")),
        (Some("mc"), &plain, Ok("mc")),
        (Some("wmc"), &plain, Ok("wmc")),
        (Some("pwmc"), &show_weights, Ok("pwmc")),
        (Some("pmc"), &t_mc_show, Ok("pmc")),
        (Some("pwmc"), &t_pwmc, Ok("pwmc")),
        (Some("compile"), &weights, Ok("compile")),
        (Some("compile"), &t_mc, Err(&["mode compile", "`c t mc` line", "declares mode mc"])),
        (Some("compile"), &show, Err(&["mode compile", "`c p show` line", "projected"])),
        (Some("mc"), &show, Err(&["mode mc", "`c p show` line", "projected"])),
        (Some("mc"), &weights, Err(&["mode mc", "`c p weight` lines", "mode wmc", "mode pwmc"])),
        (Some("mc"), &t_mc_weights, Err(&["mode mc", "`c p weight` lines", "mode wmc", "mode pwmc"])),
        (Some("pmc"), &show_weights, Err(&["mode pmc", "`c p weight` lines", "mode pwmc"])),
        (Some("pmc"), &t_pmc_weights, Err(&["mode pmc", "`c p weight` lines", "mode pwmc"])),
        (Some("mc"), &t_wmc, Err(&["mode mc", "`c t wmc` line", "declares mode wmc"])),
        (Some("wmc"), &t_mc, Err(&["mode wmc", "`c t mc` line"])),
        (Some("pmc"), &t_pwmc, Err(&["mode pmc", "`c t pwmc` line"])),
        (Some("mc"), &t_mc_show, Err(&["mode mc", "`c t mc` and `c p show` lines", "declares mode pmc"])),
    ];
    for (mode, cnf, expected) in rows {
        let mut args = vec!["--time-limit-ms", "30000"];
        args.extend(mode.map(|mode| ["--mode", mode]).iter().flatten());
        args.push(cnf.path());
        let (out, record) = run_with_record(&args);
        match expected {
            Ok(resolved) => {
                assert!(out.ok(), "{mode:?} on {}: {}", cnf.path(), out.stderr);
                assert_eq!(record["mode"], resolved, "{mode:?} on {}: {record}", cnf.path());
            }
            Err(names) => {
                assert_eq!(out.code, Some(2), "{mode:?} on {}: {}", cnf.path(), out.stderr);
                for name in names {
                    assert!(out.stderr.contains(name), "{mode:?} on {} must name {name:?}: {}", cnf.path(), out.stderr);
                }
            }
        }
    }
}

/// vitri's Arjun settings reach the weighted count's Arjun reduction: with
/// SBVA off, Arjun reduces this circuit to a different formula with the same
/// count.
#[test]
fn arjun_settings_reach_the_weighted_reduction() {
    let circuit = std::fs::read_to_string("tests/data/cnfs/LGSynth89/majority.cnf").expect("the circuit is readable");
    let weights: String = (1..=14).map(|v| format!("c p weight {v} 3/10 0\n")).collect();
    let cnf = TempCnf::new("arjun_settings_weighted", &format!("c t wmc\n{weights}{circuit}"));
    let reduction_and_count = |env: Env| {
        let out = run(&["--time-limit-ms", "20000", cnf.path()], env);
        assert!(out.ok(), "{env:?}: {}", out.stderr);
        let line = out.stderr.lines().find(|l| l.starts_with("[wmc] arjun:")).map(str::to_owned);
        (line.unwrap_or_else(|| panic!("{env:?}: no Arjun line: {}", out.stderr)), out.stdout)
    };
    let (reduction, count) = reduction_and_count(&[]);
    let (reduction_without_sbva, count_without_sbva) =
        reduction_and_count(&[("VITRI_ARJUN_SBVA", "off")]);
    assert_ne!(reduction, reduction_without_sbva, "`VITRI_ARJUN_SBVA=off` did not reach Arjun");
    assert_eq!(count, count_without_sbva);
}

/// A projected count compiles on an explicit `--vtree`, and on `portfolio`
/// without one, whether `--mode` or the header asks for it; the count does
/// not change.
#[test]
fn projected_counts_compile_on_the_vtree_given() {
    let body = "c p show 1 2 0\nc p weight 1 3 0\nc p weight -1 2 0\nc p weight 2 5 0\nc p weight -2 7 0\n\
                p cnf 3 2\n-1 -2 0\n1 2 3 0\n";
    let by_flag = TempCnf::new("pwmc_by_flag", body);
    let by_header = TempCnf::new("pwmc_by_header", &format!("c t pwmc\n{body}"));
    for (args, spec) in [
        (vec!["--mode", "pwmc", by_flag.path()], "portfolio"),
        (vec![by_header.path()], "portfolio"),
        (vec!["--mode", "pwmc", "--vtree", "flowcutter-primal", by_flag.path()], "flowcutter-primal"),
        (vec!["--vtree", "balanced", by_header.path()], "balanced"),
    ] {
        let (out, record) = run_with_record(&args);
        assert!(out.ok(), "{args:?}: {}", out.stderr);
        assert!(out.prints("projected weighted count: 45/1"), "{args:?}: {}", out.stdout);
        assert_eq!(record["mode"], "pwmc", "{args:?}: {record}");
        assert_eq!(record["vtree_spec"], spec, "{args:?}: {record}");
    }
}

// ── What it prints ───────────────────────────────────────────────────────────

/// Stdout is the answer and nothing else; the record describes the run and
/// the diagram the count was read off.
#[test]
fn stdout_is_the_answer_and_the_record_the_run() {
    let (out, record) = run_with_record(&[TINY]);
    assert!(out.ok(), "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "model count: 48");
    assert_eq!(record["mode"], "compile", "{record}");
    assert_eq!(record["exit_reason"], "solved", "{record}");
    assert_eq!(record["model_count"], "48", "{record}");
    assert_eq!(record["variables"], 6, "{record}");
    assert_eq!(record["clauses"], 3, "{record}");
    for field in ["vtree_spec", "tdd_nodes", "tdd_pairs", "tdd_width", "compile_ms"] {
        assert!(record.get(field).is_some(), "no `{field}`: {record}");
    }
    // The compile on the portfolio's pick answered, and stderr names the pick
    // the record does.
    assert_eq!(record["answered_by"], "compile", "{record}");
    let pick = record["vtree_candidate"].as_str().unwrap_or_else(|| panic!("no `vtree_candidate`: {record}"));
    assert!(out.stderr.contains(&format!("[vtree] portfolio: {pick}\n")), "{}", out.stderr);
}

/// Compile mode puts the forced variables back on the circuit in space linear
/// in their number: 100 000 of them compile under a 3 GiB address-space cap.
/// A vtree per forced variable would size each leaf's table by its variable
/// id, an expansion quadratic in their number that this cap does not hold.
#[test]
fn compile_expands_many_forced_variables_in_linear_space() {
    let n = 100_000u32;
    let mut text = format!("p cnf {n} {}\n1 2 0\n-1 3 0\n", n - 1);
    for v in 4..=n {
        text.push_str(&format!("{v} 0\n"));
    }
    let cnf = TempCnf::new("many_forced", &text);
    let out = run(&["--mode", "compile", "--mem-limit-mb", "3072", cnf.path()], &[]);
    assert!(out.ok(), "exit {:?}: {}", out.code, out.stderr);
    assert!(out.prints("model count: 4"), "{}", out.stdout);
}

/// `--save-tdd` writes the diagram, also when every variable is forced.
#[test]
fn save_tdd_writes_the_diagram() {
    let all_backbone = TempCnf::new("save_all_backbone", "p cnf 3 3\n1 0\n2 0\n3 0\n");
    for (cnf, answer, lines) in
        [(TINY, "model count: 48", &["p tdd", "L ", "I "][..]), (all_backbone.path(), "model count: 1", &["p tdd", "L "])]
    {
        let saved = common::tmp_path("saved.tdd");
        let out = run(&["--save-tdd", saved.to_str().unwrap(), cnf], &[]);
        assert!(out.ok(), "{cnf}: {}", out.stderr);
        assert!(out.prints(answer), "{cnf}: {}", out.stdout);
        let content = std::fs::read_to_string(&saved).expect("the diagram file exists");
        let _ = std::fs::remove_file(&saved);
        let _ = std::fs::remove_file(format!("{}.vtree", saved.display()));
        for line in lines {
            assert!(content.contains(line), "{cnf}: no {line:?} line in the saved diagram:\n{content}");
        }
    }
}

/// Without `--vtree`, a model or weighted count runs its cascade, whether
/// `--mode` or a header asked for the count; the record names the vtree the
/// cascade compiles on, and a model count's cascade records its Arjun time.
#[test]
fn counts_run_the_cascade_without_vtree() {
    let (out, record) = run_with_record(&["tests/data/wmc_header_tiny_sat.cnf"]);
    assert!(out.ok(), "{}", out.stderr);
    assert!(out.stdout.contains("weighted count"), "{}", out.stdout);
    assert_eq!(record["vtree_spec"], "portfolio", "{record}");
    assert!(out.stderr.contains("[wmc] preprocessing:"), "{}", out.stderr);

    // 1 ∨ 2 ∨ 3: seven models, by the header and by the flag.
    for args in [vec!["tests/data/mc_header_tiny.cnf"], vec!["--mode", "mc", "tests/data/mc_header_tiny.cnf"]] {
        let (out, record) = run_with_record(&args);
        assert!(out.ok(), "{args:?}: {}", out.stderr);
        assert!(out.prints("model count: 7"), "{args:?}: {}", out.stdout);
        assert_eq!(record["vtree_spec"], "portfolio", "{args:?}: {record}");
        assert!(record.get("arjun_ms").is_some(), "{args:?}: {record}");
    }

    // A CNF without a header counts through the cascade too.
    let (out, record) = run_with_record(&["--mode", "mc", TINY]);
    assert!(out.ok(), "{}", out.stderr);
    assert!(out.prints("model count: 48"), "{}", out.stdout);
    assert!(record.get("arjun_ms").is_some(), "{record}");
}

/// `--mode mc --vtree portfolio` streams through vitri's simplify-only
/// preprocessing, without the cascade and without Arjun, and an instance that
/// preprocessing refutes returns before any vtree is built, answered by
/// preprocessing.
#[test]
fn streaming_mc_runs_the_simplify_only_frontend() {
    for (cnf, answer, builds_a_vtree, answered_by) in [
        (TINY, "model count: 48", true, "compile"),
        ("tests/data/unsat_empty_clause.cnf", "model count: 0", false, "preprocessing"),
    ] {
        let (out, record) = run_with_record(&["--mode", "mc", "--vtree", "portfolio", cnf]);
        assert!(out.ok(), "{cnf}: {}", out.stderr);
        assert!(out.prints(answer), "{cnf}: {}", out.stdout);
        assert_eq!(record["answered_by"], answered_by, "{cnf}: {record}");
        assert!(out.stderr.contains("[mc] preprocessing:"), "{cnf}: {}", out.stderr);
        assert!(record.get("arjun_ms").is_none() && record.get("reduction").is_none(), "{cnf}: {record}");
        if !builds_a_vtree {
            assert!(!out.stderr.contains("[vtree]") && !out.stderr.contains("compile: start"), "{cnf}: {}", out.stderr);
        }
    }
}

/// Streaming MC detects the reduced formula's components once, when the root
/// compile splits it.
#[test]
fn streaming_mc_detects_components_once() {
    // Two copies of a 4-variable formula with 9 models.
    let two = TempCnf::new("two_components", "p cnf 8 6\n1 2 3 0\n-1 -2 0\n-2 -3 4 0\n5 6 7 0\n-5 -6 0\n-6 -7 8 0\n");
    let out = run(&["--mode", "mc", "--vtree", "portfolio", two.path()], &[]);
    assert!(out.prints("model count: 81"), "{}", out.stdout);
    assert!(out.stderr.contains("[vtree] portfolio\n"), "{}", out.stderr);
    assert_eq!(out.stderr.matches("[vtree] 2 independent components").count(), 1, "{}", out.stderr);
}

/// `TIDIDI_SAT_PRUNE=off` turns SAT pruning off, and the record says so in
/// its `sat_prune` key and carries none of the pass's tallies; the count is
/// the same. `on` is the default, and the record then has no `sat_prune` key.
/// A pass fires on this instance by default, in the compile and in the model
/// count's cascade. The hour-long limit is the protocol row's: a pass stops
/// at a share of the time limit, so a shorter one could stop it before it
/// fires on a slow machine.
#[test]
fn sat_prune_off_skips_the_pass_and_keeps_the_count() {
    let cnf = "tests/data/cnfs/satlib/uf100/uf100-01000.cnf";
    let default: Env = &[];
    let on: Env = &[("TIDIDI_SAT_PRUNE", "on")];
    let off: Env = &[("TIDIDI_SAT_PRUNE", "off")];
    let tallies = |record: &serde_json::Value| -> Vec<String> {
        record.as_object().map_or(Vec::new(), |keys| {
            keys.keys().filter(|key| key.starts_with("sat_prune_")).cloned().collect()
        })
    };
    for mode in ["compile", "mc"] {
        let args = ["--mode", mode, "--time-limit-ms", "3600000", cnf];
        for env in [default, on, off] {
            let (out, record) = run_with_record_in(&args, env);
            assert!(out.ok(), "{mode} {env:?}: {}", out.stderr);
            assert!(out.prints("model count: 16963"), "{mode} {env:?}: {}", out.stdout);
            if env == off {
                assert_eq!(record["sat_prune"], "off", "{mode}: {record}");
                assert!(tallies(&record).is_empty(), "{mode}: the pass ran with the switch off: {record}");
            } else {
                assert!(record.get("sat_prune").is_none(), "{mode} {env:?}: {record}");
                assert!(
                    record["sat_prune_passes_triggered"].as_u64().is_some_and(|passes| passes > 0),
                    "{mode} {env:?}: no pass fired, so the instance does not test the switch: {record}"
                );
            }
        }
    }
}

/// The Arjun phase must not build a FlowCutter vtree on the whole pre-Arjun
/// formula: FlowCutter allocates an n×n bit matrix, which on ~1M variables
/// exhausts memory.
///
/// 1,000 disjoint 200-variable chains, whose whole-formula matrix busts the
/// 3 GB address-space cap while each component's fits easily. The run's own
/// one-minute time limit falls well past where a whole-formula build would
/// fail, and it must end by answering or by giving up.
#[test]
#[ignore = "slow: runs up to its one-minute time limit"]
fn the_cascade_splits_a_huge_formula_before_building_a_vtree() {
    let (components, size) = (1_000usize, 200usize);
    let mut text = format!("p cnf {} {}\n", components * size, components * (size - 1));
    for c in 0..components {
        for j in 1..size {
            text.push_str(&format!("{} {} 0\n", c * size + j, c * size + j + 1));
        }
    }
    let cnf = TempCnf::new("huge_chains", &text);
    drop(text);
    let output = Command::new("bash")
        .arg("-c")
        .arg(format!("ulimit -v 3145728; timeout 300 '{BINARY}' --mode mc --time-limit-ms 60000 '{}'", cnf.path()))
        .output()
        .expect("bash runs");
    // 0: answered; 1: gave up at the time limit. A failed stage, a run past
    // its own limit, or a crash on a refused allocation is the blowup.
    let code = output.status.code();
    assert!(matches!(code, Some(0 | 1)), "exit {code:?}: {}", String::from_utf8_lossy(&output.stderr));
}

// ── The wall ─────────────────────────────────────────────────────────────────

/// A random 3-CNF over 400 variables and 1500 clauses, near the
/// satisfiability threshold, as DIMACS text after the lines `header`. With
/// `show` it shows variables `1..=200`, and with `weighted`, `w(v) = 2` and
/// `w(¬v) = 1` on them. The seed is fixed, so every run reads the same
/// clauses.
fn random_3cnf_text(header: &str, show: bool, weighted: bool) -> String {
    let formula = common::random_3cnf(400, 1500, 1);
    let mut text = header.to_owned();
    if show {
        text.push_str(&format!("c p show {} 0\n", (1..=200).map(|v| v.to_string()).collect::<Vec<_>>().join(" ")));
    }
    if weighted {
        for v in 1..=200 {
            text.push_str(&format!("c p weight {v} 2 0\nc p weight -{v} 1 0\n"));
        }
    }
    text.push_str(&format!("p cnf {} {}\n", formula.num_vars(), formula.clauses().len()));
    for clause in formula.clauses() {
        for literal in &clause.literals {
            text.push_str(&format!("{} ", literal.to_dimacs()));
        }
        text.push_str("0\n");
    }
    text
}

/// A projected count ends near `--time-limit-ms`, answered or not. The show-set
/// enumeration, the conditioning recovery and the definability oracle each
/// have a budget of their own, and none may run past the run's wall. The 8 s
/// bound leaves room for the vtree construction and the compile's SAT
/// pruning, which can end a little past the wall.
#[test]
fn projected_counts_end_at_the_wall() {
    for (task, weighted) in [("pmc", false), ("pwmc", true)] {
        let cnf = TempCnf::new(&format!("wall_{task}"), &random_3cnf_text(&format!("c t {task}\n"), true, weighted));
        let started = std::time::Instant::now();
        let output = Command::new("timeout")
            .args(["60", BINARY, "--time-limit-ms", "2000", cnf.path()])
            .output()
            .expect("timeout runs the binary");
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "{task}: ran {elapsed:?} on a 2 s wall (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A run that gives up exits 1 with no answer on stdout, and its record says
/// `incomplete`, holds no count and no verdict, whatever the task, and the
/// last line of stderr says what ended the run. A 1 ms wall on a random 3-CNF
/// of 400 variables leaves every task without a count, and every run ends
/// soon after it; a 0 ms wall leaves even a tiny instance, which the show-set
/// enumeration would answer at once, without one.
#[test]
fn a_give_up_exits_1_on_every_task() {
    let plain = TempCnf::new("give_up_plain", &random_3cnf_text("", false, false));
    let weighted = TempCnf::new("give_up_weighted", &random_3cnf_text("", false, true));
    let shown = TempCnf::new("give_up_shown", &random_3cnf_text("", true, false));
    let shown_weighted = TempCnf::new("give_up_shown_weighted", &random_3cnf_text("", true, true));
    // Whole-run expiry has one diagnostic, regardless of the active stage.
    let last_line = "error: no answer: the time limit was reached";
    for (task, flags, wall, cnf) in [
        ("compile", &["--mode", "compile"][..], "1", plain.path()),
        ("mc", &["--mode", "mc"], "1", plain.path()),
        ("mc", &["--mode", "mc", "--vtree", "portfolio"], "1", plain.path()),
        ("wmc", &["--mode", "wmc"], "1", weighted.path()),
        ("pmc", &["--mode", "pmc"], "1", shown.path()),
        ("pwmc", &["--mode", "pwmc"], "1", shown_weighted.path()),
        ("pmc", &[], "0", "tests/data/pmc_tiny_show.cnf"),
    ] {
        let started = std::time::Instant::now();
        let (out, record) = run_with_record(&[flags, &["--time-limit-ms", wall, cnf]].concat());
        let elapsed = started.elapsed();
        assert_eq!(out.code, Some(1), "{flags:?}: {}", out.stderr);
        assert!(elapsed < std::time::Duration::from_secs(20), "{flags:?}: ran {elapsed:?} on a {wall} ms wall");
        assert_eq!(out.stdout, "", "{flags:?}");
        assert!(
            out.stderr.lines().last().is_some_and(|line| line.starts_with(last_line)),
            "{flags:?} must end on {last_line:?}: {}",
            out.stderr
        );
        assert_eq!(record["mode"], task, "{flags:?}: {record}");
        assert_eq!(record["exit_reason"], "incomplete", "{flags:?}: {record}");
        assert!(record.get("variables").is_some(), "{flags:?}: the record describes the input: {record}");
        for field in ["model_count", "weighted_count", "satisfiable"] {
            assert!(record.get(field).is_none(), "{flags:?}: {record}");
        }
    }
}

// ── Counts ───────────────────────────────────────────────────────────────────

/// Show variables `1..=count` chained by binary clauses, and a projected
/// variable forced both ways.
fn chained_show_with_a_contradiction(count: usize) -> String {
    let show: Vec<String> = (1..=count).map(|v| v.to_string()).collect();
    let mut text = format!("c t pmc\nc p show {} 0\np cnf {} {}\n", show.join(" "), count + 1, count + 1);
    for v in 1..count {
        text.push_str(&format!("{v} {} 0\n", v + 1));
    }
    text + &format!("{0} 0\n-{0} 0\n", count + 1)
}

/// Exactly one of the show variables `1..=17`, and a projected variable 18
/// kept in play by `18 ∨ 1`. Seventeen constrained show variables are past
/// the dense sweep's cap of sixteen, so the count goes to the projected
/// AllSAT enumerator; seventeen feasible projections enumerate in
/// milliseconds, so it does not run out of wall and fall through to the
/// compile path. With `weighted`, `w(v) = 2` and `w(¬v) = 1` on the show
/// variables.
fn exactly_one_of_17(header: &str, weighted: bool) -> String {
    let mut text = format!("{header}\nc p show {} 0\n", (1..=17).map(|v| v.to_string()).collect::<Vec<_>>().join(" "));
    if weighted {
        for v in 1..=17 {
            text.push_str(&format!("c p weight {v} 2 0\nc p weight -{v} 1 0\n"));
        }
    }
    let mut clauses = vec![(1..=17).map(|v| format!("{v} ")).collect::<String>() + "0"];
    for i in 1..=17 {
        for j in i + 1..=17 {
            clauses.push(format!("-{i} -{j} 0"));
        }
    }
    clauses.push("18 1 0".into());
    text + &format!("p cnf 18 {}\n{}\n", clauses.len(), clauses.join("\n"))
}

/// The chain 1 → 2 → … → `n` over the show variables `1..=n`, each link
/// through a hidden variable, weighing only the positive literals of 1 (3/10)
/// and `n` (7/10). The projections switch on a suffix of the chain, so the
/// count is 3/10 · 7/10 + (`n` − 1) · 7/10 + 1. More show variables than the
/// enumeration takes send the run to the compile.
fn weighted_chain(n: u32) -> String {
    let show = (1..=n).map(|v| v.to_string()).collect::<Vec<_>>().join(" ");
    let mut text = format!("c t pwmc\np cnf {} {}\nc p show {show} 0\nc p weight 1 3/10 0\nc p weight {n} 7/10 0\n", 2 * n - 1, 2 * (n - 1));
    for v in 1..n {
        text.push_str(&format!("-{v} {0} 0\n-{0} {1} 0\n", n + v, v + 1));
    }
    text
}

/// The binary on `args` and `cnf` prints `answer` as a line of stdout, and
/// `method` on stderr when given. A `cnf` with a newline in it is DIMACS
/// text, written to a temporary file named after the test.
fn check_count(name: &str, args: &[&str], cnf: &str, answer: &str, method: Option<&str>) {
    let file;
    let cnf = if cnf.contains('\n') {
        file = TempCnf::new(name, cnf);
        file.path()
    } else {
        cnf
    };
    let out = run(&[args, &[cnf]].concat(), &[]);
    assert!(out.ok(), "{name}: {}", out.stderr);
    assert!(out.prints(answer), "{name}: expected {answer:?} in\n{}", out.stdout);
    if let Some(method) = method {
        assert!(out.stderr.contains(method), "{name}: expected {method:?} in\n{}", out.stderr);
    }
}

/// One test per row: `name: [args] cnf => answer[, method];`.
macro_rules! counts {
    ($( $(#[$attr:meta])* $name:ident: [$($arg:expr),*] $cnf:expr => $answer:literal $(, $method:literal)?; )*) => {
        $(
            $(#[$attr])*
            #[test]
            fn $name() {
                check_count(stringify!($name), &[$($arg),*], &$cnf, $answer, None $(.or(Some($method)))?);
            }
        )*
    };
}

counts! {
    /// Exactly one of 1, 2, 3, and 4 and 5 in no clause: 3 · 2².
    one_hot_beside_free_variables: ["--mode", "mc"] "tests/data/onehot_free_residual.cnf" => "model count: 12";
    /// 1 ↔ 2 and 3 ∨ 4: 2 · 3.
    equivalences_through_full_preprocessing: ["--preprocess", "full"] "p cnf 4 3\n-1 2 0\n1 -2 0\n3 4 0\n"
        => "model count: 6";
    /// Unsatisfiable once equivalences are substituted.
    unsat_after_equivalences: ["--preprocess", "full"] "tests/data/cnfs/pmc/Network/Ratio_75/75-10-3-q.cnf" => "model count: 0";
    unsat_by_propagation: [] "p cnf 1 2\n1 0\n-1 0\n" => "model count: 0";
    /// Backbone probing forces every variable of this counter circuit.
    all_backbone: [] "tests/data/cnfs/pmc/bmc/cnt06.shuffled.cnf" => "model count: 1", "every variable is a backbone";
    all_backbone_mc: ["--mode", "mc", "--vtree", "portfolio"] "tests/data/cnfs/pmc/bmc/cnt06.shuffled.cnf"
        => "model count: 1", "every variable is a backbone";
    /// A SAT-pruning pass fires on this instance under a FlowCutter primal
    /// vtree: it enumerates models under blocking clauses, retires them
    /// before it probes the nodes no model reached, and finds hundreds of
    /// those unsatisfiable. A probe that wrongly came out unsatisfiable would
    /// prune a live node, and the count would come out low.
    sat_prune_blockers_stay_off_during_probes: ["--mode", "mc", "--vtree", "flowcutter-primal"]
        "tests/data/cnfs/satlib/uf100/uf100-01000.cnf" => "model count: 16963";

    // Compile mode through preprocessing, each count by brute force.
    identity_map: ["--preprocess", "none", "--vtree", "portfolio"] "p cnf 3 3\n1 2 -3 0\n-1 3 0\n2 -3 0\n"
        => "model count: 4";
    backbone_map: ["--preprocess", "full", "--vtree", "portfolio"] "p cnf 4 4\n1 0\n-4 0\n2 3 0\n-2 3 0\n"
        => "model count: 2";
    free_variable_map: ["--preprocess", "full", "--vtree", "portfolio"] "p cnf 5 3\n1 2 0\n-2 3 0\n-1 -3 0\n"
        => "model count: 8";
    equivalence_map: ["--preprocess", "full", "--vtree", "portfolio"]
        "p cnf 4 5\n-1 2 0\n1 -2 0\n-2 3 0\n2 -3 0\n1 4 0\n" => "model count: 3";
    anti_equivalence_map: ["--preprocess", "full", "--vtree", "portfolio"]
        "p cnf 4 4\n1 2 0\n-1 -2 0\n2 3 0\n-2 4 0\n" => "model count: 4";
    combined_map: ["--preprocess", "full", "--vtree", "portfolio"]
        "p cnf 6 6\n1 0\n3 4 0\n-3 -4 0\n2 3 0\n-2 5 0\n-5 3 0\n" => "model count: 6";
    all_backbone_map: ["--preprocess", "full", "--vtree", "portfolio"] "p cnf 4 4\n1 0\n-2 0\n3 0\n-4 0\n"
        => "model count: 1";
    all_free_map: ["--preprocess", "full", "--vtree", "portfolio"] "p cnf 4 0\n" => "model count: 16";

    /// c ↔ (a ∧ b), all three weighted. Arjun drops c from the show set
    /// because a and b define it; its weight still counts, or the answer is
    /// 1/1.
    pwmc_defined_variable_keeps_its_weight: [] "tests/data/pwmc_arjun_defined_weight.cnf"
        => "projected weighted count: 91/125";
    /// Show 1, weighted to sum one; projected 2, forced true, weighted 1/5
    /// and 4/5. A projected variable weighs one, or the answer is 1/5.
    pwmc_projected_variable_weighs_one: [] "tests/data/pwmc_projected_var_weight.cnf"
        => "projected weighted count: 1/1";
    /// Show 1 free, only its positive literal weighed: the negative one
    /// weighs 1 - 3/10, so 3/10 + 7/10, not the 3/10 + 1 of weighing a
    /// missing literal 1.
    pwmc_unnamed_negative_literal_weighs_one_minus_the_positive: []
        "c t pwmc\np cnf 1 0\nc p show 1 0\nc p weight 1 3/10 0\n" => "projected weighted count: 1/1";
    pwmc_unnamed_positive_literal_weighs_one_minus_the_negative: []
        "c t pwmc\np cnf 1 0\nc p show 1 0\nc p weight -1 3/10 0\n" => "projected weighted count: 1/1";
    /// Show 1 over projected 2, 1 ∨ 2: 1 true weighs 3/10, and 1 false
    /// extends through 2 and weighs 7/10.
    pwmc_unnamed_literal_beside_a_projected_variable: []
        "c t pwmc\np cnf 2 1\nc p show 1 0\nc p weight 1 3/10 0\n1 2 0\n" => "projected weighted count: 1/1";
    /// The compile path, each weighted variable weighed on its positive
    /// literal alone, so its negative literal weighs the rest of 1. The show
    /// assignments are all false, 7/10 · 3/10, all true, 3/10 · 7/10, and the
    /// 1099 that turn true after the first variable, 7/10 · 7/10 each.
    pwmc_unnamed_literals_on_the_compile_path: [] weighted_chain(1100)
        => "projected weighted count: 53893/100", "[pwmc] compile: preprocessing";
    /// Projected 2 forced both ways: the residual has no variables.
    pmc_empty_residual_unsat: ["--time-limit-ms", "30000"] "c t pmc\nc p show 1 0\np cnf 2 2\n2 0\n-2 0\n"
        => "projected model count: 0";
    /// Projected 3 forced, and show 1 and 2 in no clause: 2².
    pmc_empty_residual_free_show: ["--time-limit-ms", "30000"] "c t pmc\nc p show 1 2 0\np cnf 3 2\n3 0\n3 0\n"
        => "projected model count: 4";
    /// 300 constrained show variables are past both enumeration ceilings, so
    /// the contradiction reaches the compile path.
    pmc_empty_residual_on_the_compile_path: ["--time-limit-ms", "30000"] chained_show_with_a_contradiction(300)
        => "projected model count: 0";
    pwmc_empty_residual_unsat: ["--time-limit-ms", "30000"]
        "c t pwmc\nc p show 1 0\nc p weight 1 3 0\nc p weight -1 2 0\np cnf 2 2\n2 0\n-2 0\n"
        => "projected weighted count: 0/1";
    /// Equal literal weights make the count the model count times ∏ w, which
    /// the integer streaming engine computes: 3 · 2².
    pwmc_symmetric_weights_count_through_mc: ["--time-limit-ms", "30000"]
        "c t pwmc\nc p show 1 2 0\nc p weight 1 2 0\nc p weight -1 2 0\nc p weight 2 2 0\nc p weight -2 2 0\np cnf 2 1\n-1 -2 0\n"
        => "projected weighted count: 12/1", "[pwmc] symmetric-subsumption: counted";
    /// (¬1 ∨ ¬2) ∧ (1 ∨ 2 ∨ 3), show 1 and 2: three of the four show
    /// assignments extend, a count no power of two gives.
    pmc_small_show_enumeration: ["--time-limit-ms", "30000"] "c t pmc\nc p show 1 2 0\np cnf 3 2\n-1 -2 0\n1 2 3 0\n"
        => "projected model count: 3", "[pmc] enumerate-show: small-show enumeration";
    /// The same weighted 3, 2 on 1 and 5, 7 on 2: 3·7 + 2·5 + 2·7.
    pwmc_small_show_enumeration: ["--time-limit-ms", "30000"]
        "c t pwmc\nc p show 1 2 0\nc p weight 1 3 0\nc p weight -1 2 0\nc p weight 2 5 0\nc p weight -2 7 0\np cnf 3 2\n-1 -2 0\n1 2 3 0\n"
        => "projected weighted count: 45/1", "[pwmc] enumerate-show: small-show enumeration";
    pmc_projected_allsat: ["--time-limit-ms", "30000"] exactly_one_of_17("c t pmc", false)
        => "projected model count: 17", "[pmc] enumerate-show: projected ALL-SAT";
    /// Seventeen projections of weight 2 each.
    pwmc_projected_allsat: ["--time-limit-ms", "30000"] exactly_one_of_17("c t pwmc", true)
        => "projected weighted count: 34/1", "[pwmc] enumerate-show: projected ALL-SAT";

    /// A bare `0` is the empty clause, so every mode counts zero.
    empty_clause_compile: ["--mode", "compile"] "p cnf 3 3\n1 2 0\n0\n-1 3 0\n" => "model count: 0";
    empty_clause_mc: ["--mode", "mc"] "p cnf 3 3\n1 2 0\n0\n-1 3 0\n" => "model count: 0";
    empty_clause_wmc: []
        "c t wmc\np cnf 3 3\nc p weight 1 0.7 0\nc p weight -1 0.3 0\n1 2 0\n0\n-1 3 0\n" => "weighted count: 0";
    empty_clause_pmc: [] "c t pmc\np cnf 3 3\nc p show 1 2 0\n1 2 0\n0\n-1 3 0\n"
        => "projected model count: 0";
    empty_clause_pwmc: []
        "c t pwmc\np cnf 3 3\nc p show 1 2 0\nc p weight 1 0.7 0\n1 2 0\n0\n-1 3 0\n" => "projected weighted count: 0/1";
}

/// A `--save-tdd` circuit reloads through the library from what the binary
/// wrote alone: the diagram file, and its vtree beside it at `<file>.vtree`,
/// which the confirmation line names. The reloaded circuit counts what the
/// binary printed, free variables included.
#[test]
fn save_tdd_reloads_through_the_library() {
    use std::sync::Arc;
    for cnf in ["tests/data/cnfs/iscas89/s641.scan.cnf", "tests/data/onehot_free_residual.cnf"] {
        let saved = common::tmp_path("reload.tdd");
        let saved_vtree = common::tmp_path("reload.tdd.vtree");
        let out = run(&["--save-tdd", saved.to_str().unwrap(), cnf], &[]);
        assert!(out.ok(), "{cnf}: {}", out.stderr);
        let printed = out
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("model count: "))
            .unwrap_or_else(|| panic!("{cnf}: no count line:\n{}", out.stdout))
            .to_string();
        assert!(
            out.stdout.contains(saved_vtree.to_str().unwrap()),
            "{cnf}: the confirmation line does not name the vtree file:\n{}",
            out.stdout
        );
        let vtree_text = std::fs::read_to_string(&saved_vtree);
        let reloaded = vtree_text.map(|text| {
            let vtree = Arc::new(tididi::Vtree::from_text(&text).expect("the saved vtree parses"));
            tididi::io::load_tdd(&saved, &vtree).expect("the saved diagram reads over its vtree")
        });
        let _ = std::fs::remove_file(&saved);
        let _ = std::fs::remove_file(&saved_vtree);
        let reloaded = reloaded.unwrap_or_else(|e| panic!("{cnf}: no vtree file beside the diagram: {e}"));
        assert_eq!(reloaded.model_count().expect("the reloaded circuit counts").to_string(), printed, "{cnf}");
    }
}

// ── The MCC answer ───────────────────────────────────────────────────────────

/// `--output-format mcc` prints the MCC output format's answer (data format
/// v1.2 §3) and nothing else on stdout, on every task: the status line, the
/// type, the log10 estimate and the answer line, in that order, or
/// `s UNKNOWN` alone when the run gives up. The status is the solver's
/// verdict, not the count's, so a satisfiable weighted instance whose weights
/// make its count zero is satisfiable, and the estimate of a negative weighted
/// count is the `neglog10` line. The record's `satisfiable` is the same
/// verdict. A 1 ms wall on a random 3-CNF of 400 variables makes each
/// counting task give up.
#[test]
fn mcc_output_prints_the_mcc_answer() {
    let give_up = |task: &str, show: bool, weighted: bool| {
        TempCnf::new(&format!("mc_format_give_up_{task}"), &random_3cnf_text(&format!("c t {task}\n"), show, weighted))
    };
    let (mc_hard, wmc_hard, pmc_hard, pwmc_hard) =
        (give_up("mc", false, false), give_up("wmc", false, true), give_up("pmc", true, false), give_up("pwmc", true, true));
    let compile_hard = TempCnf::new("mc_format_give_up_compile", &random_3cnf_text("", false, false));
    let wall = ["--time-limit-ms", "1"];
    // (task and case, flags, instance, stdout)
    let rows: [(&str, &[&str], &str, &str); 19] = [
        (
            "mc, satisfiable: the data format's Example 1",
            &[],
            "tests/data/mcc_mc_sat.cnf",
            "s SATISFIABLE\nc s type mc\nc s log10-estimate 1.3424226808222062\nc s exact arb int 22\n",
        ),
        (
            "mc, unsatisfiable",
            &[],
            "tests/data/mcc_mc_unsat.cnf",
            "s UNSATISFIABLE\nc s type mc\nc s log10-estimate -inf\nc s exact arb int 0\n",
        ),
        ("mc, gives up", &wall, mc_hard.path(), "s UNKNOWN\n"),
        (
            "plain compile, satisfiable: its count is a model count",
            &[],
            TINY,
            "s SATISFIABLE\nc s type mc\nc s log10-estimate 1.6812412373755872\nc s exact arb int 48\n",
        ),
        (
            "plain compile, unsatisfiable",
            &[],
            "tests/data/pigeon4_3.cnf",
            "s UNSATISFIABLE\nc s type mc\nc s log10-estimate -inf\nc s exact arb int 0\n",
        ),
        ("plain compile, gives up", &wall, compile_hard.path(), "s UNKNOWN\n"),
        (
            "wmc, satisfiable",
            &[],
            "tests/data/wmc_header_tiny_sat.cnf",
            "s SATISFIABLE\nc s type wmc\nc s log10-estimate 0.5185139398778873\nc s exact double prec-sci 3.2999999999999985e0\n",
        ),
        (
            "wmc, unsatisfiable: refuted in preprocessing",
            &[],
            "tests/data/wmc_unsat.cnf",
            "s UNSATISFIABLE\nc s type wmc\nc s log10-estimate -inf\nc s exact double prec-sci 0\n",
        ),
        (
            "wmc, satisfiable with count 0: a variable weighed 0 both ways",
            &[],
            "tests/data/mcc_wmc_zero.cnf",
            "s SATISFIABLE\nc s type wmc\nc s log10-estimate -inf\nc s exact double prec-sci 0\n",
        ),
        (
            "wmc, satisfiable with count 0: weights that cancel",
            &[],
            "tests/data/mcc_wmc_cancel.cnf",
            "s SATISFIABLE\nc s type wmc\nc s log10-estimate -inf\nc s exact double prec-sci 0\n",
        ),
        (
            "wmc, negative count -3/8",
            &[],
            "tests/data/mcc_wmc_negative.cnf",
            "s SATISFIABLE\nc s type wmc\nc s neglog10-estimate -0.4259687322722811\nc s exact double prec-sci -3.75e-1\n",
        ),
        ("wmc, gives up", &wall, wmc_hard.path(), "s UNKNOWN\n"),
        (
            "pmc, satisfiable: the data format's Example 4",
            &[],
            "tests/data/mcc_pmc_sat.cnf",
            "s SATISFIABLE\nc s type pmc\nc s log10-estimate 0.47712125471966244\nc s exact arb int 3\n",
        ),
        (
            "pmc, unsatisfiable",
            &[],
            "tests/data/mcc_pmc_unsat.cnf",
            "s UNSATISFIABLE\nc s type pmc\nc s log10-estimate -inf\nc s exact arb int 0\n",
        ),
        ("pmc, gives up", &wall, pmc_hard.path(), "s UNKNOWN\n"),
        (
            "pwmc, satisfiable",
            &[],
            "tests/data/mcc_pwmc_sat.cnf",
            "s SATISFIABLE\nc s type pwmc\nc s log10-estimate -0.055517327849831544\nc s exact arb frac 22/25\n",
        ),
        (
            "pwmc, unsatisfiable",
            &[],
            "tests/data/mcc_pwmc_unsat.cnf",
            "s UNSATISFIABLE\nc s type pwmc\nc s log10-estimate -inf\nc s exact arb frac 0/1\n",
        ),
        (
            "pwmc, satisfiable with count 0: a show variable weighed 0 both ways",
            &[],
            "tests/data/mcc_pwmc_zero.cnf",
            "s SATISFIABLE\nc s type pwmc\nc s log10-estimate -inf\nc s exact arb frac 0/1\n",
        ),
        (
            "pwmc, negative count -1/4",
            &[],
            "tests/data/mcc_pwmc_negative.cnf",
            "s SATISFIABLE\nc s type pwmc\nc s neglog10-estimate -0.6020599913279624\nc s exact arb frac -1/4\n",
        ),
    ];
    for (case, flags, cnf, stdout) in rows {
        let (out, record) = run_with_record(&[flags, &["--output-format", "mcc", cnf]].concat());
        assert_eq!(out.stdout, stdout, "{case}: {}", out.stderr);
        let status = stdout.lines().next().expect("every row has a status line");
        let (code, satisfiable) = match status {
            "s SATISFIABLE" => (0, Some(true)),
            "s UNSATISFIABLE" => (0, Some(false)),
            _ => (1, None),
        };
        assert_eq!(out.code, Some(code), "{case}: {}", out.stderr);
        assert_eq!(record.get("satisfiable").and_then(|v| v.as_bool()), satisfiable, "{case}: {record}");
    }
    // `s UNKNOWN` is the transcript's answer, so it precedes a record written
    // to stdout, as an answer does.
    let out = run(&["--output-format", "mcc", "--time-limit-ms", "1", "--stats", "-", pwmc_hard.path()], &[]);
    assert_eq!(out.code, Some(1), "pwmc, gives up: {}", out.stderr);
    let mut lines = out.stdout.lines();
    assert_eq!(lines.next(), Some("s UNKNOWN"), "{}", out.stdout);
    assert!(lines.next().is_some_and(|line| line.contains(r#""exit_reason":"incomplete""#)), "{}", out.stdout);
    assert_eq!(lines.next(), None, "{}", out.stdout);
}
