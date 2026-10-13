//! The fixture protocol: what the binary prints for a fixed set of instances.
//!
//! Each row runs the binary once and compares its exit status, stdout and
//! stderr — the `--stats` record included — against a checked-in file under
//! `tests/data/protocol/`. Between them the rows cover every mode
//! (`--mode compile`, `mc`, `wmc`, `pmc` and `pwmc`) and both integer-MC
//! entries: `--mode mc` without `--vtree` runs the cascade, and with
//! `--vtree` the streaming entry, `run_mc_streaming` in `src/stage/mc/mod.rs`.
//!
//! This is a transcript pin, not a correctness check. It says a refactor left
//! the counts, the stage that answered and the candidate, the probe and node
//! tallies and the diagnostics exactly as they were; `tests/correctness.rs`
//! is what says the counts are right in the first place.
//!
//! Regenerate after an intended output change with
//!
//! ```text
//! UPDATE_PROTOCOL=1 cargo test --test protocol -- --include-ignored
//! ```
//!
//! and read the diff before committing it.

use std::path::PathBuf;
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_tididi");

/// Every row runs with the same time limit, so a row that consults the clock
/// consults the same one on every machine.
const TIME_LIMIT_MS: &str = "60000";

/// The env var that rewrites the expected files instead of comparing them.
const UPDATE_VAR: &str = "UPDATE_PROTOCOL";

/// The command that rewrites the expected files.
fn regenerate() -> String {
    format!("{UPDATE_VAR}=1 cargo test --test protocol -- --include-ignored")
}

/// Replace the wall-clock carriers in one line of output, and only those.
///
/// Two things in this output move from run to run, both of them clock
/// readings; everything else — counts, sizes, stage and candidate names, probe
/// and node tallies, preprocessing shapes — is compared literally.
///
/// 1. A `--stats` field whose key ends in `_ms` or `_us`: its number becomes
///    the string `"<t>"`.
/// 2. A duration in prose, written `123ms`, `123 ms` or `123us`: becomes `<t>`.
fn blank_durations(line: &str) -> String {
    let after_keys = blank_stat_fields(line);
    blank_prose_durations(&after_keys)
}

/// `"preprocess_ms":14` -> `"preprocess_ms":"<t>"`, for any `_ms`/`_us` key.
fn blank_stat_fields(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        // A stat key ends `_ms":` or `_us":` — both 5 bytes — optionally
        // followed by spaces.
        let key_end = line[i..]
            .find("_ms\":")
            .into_iter()
            .chain(line[i..].find("_us\":"))
            .min();
        let Some(offset) = key_end else {
            out.push_str(&line[i..]);
            break;
        };
        let value_start = i + offset + 5;
        out.push_str(&line[i..value_start]);
        let mut j = value_start;
        while bytes.get(j) == Some(&b' ') {
            out.push(' ');
            j += 1;
        }
        let number_end = scan_number(bytes, j);
        if number_end > j {
            out.push_str("\"<t>\"");
            i = number_end;
        } else {
            // Not a bare number (already blanked, or a string) — leave it be.
            i = j;
        }
    }
    out
}

/// `compile 99ms` -> `compile <t>`, `vtree 177 ms` -> `vtree <t>`.
fn blank_prose_durations(line: &str) -> String {
    // Byte-wise, because the diagnostics carry non-ASCII (`374c\u{2192}114c`);
    // every cut below lands on an ASCII digit or unit, so the bytes copied
    // through stay a well-formed string.
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() || starts_inside_a_token(bytes, i) {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let number_end = scan_number(bytes, i);
        let mut j = number_end;
        if bytes.get(j) == Some(&b' ') {
            j += 1;
        }
        let unit = ["ms", "us"]
            .into_iter()
            .find(|u| bytes[j..].starts_with(u.as_bytes()))
            .filter(|u| !is_token_char(bytes.get(j + u.len()).copied()));
        match unit {
            Some(u) => {
                out.extend_from_slice(b"<t>");
                i = j + u.len();
            }
            None => {
                out.extend_from_slice(&bytes[i..number_end]);
                i = number_end;
            }
        }
    }
    String::from_utf8(out).expect("cuts land on ASCII boundaries")
}

/// True when the digit at `i` continues an identifier or a larger number
/// (`mc2025_025`, `8.5875e1`), so it does not begin a duration.
fn starts_inside_a_token(bytes: &[u8], i: usize) -> bool {
    i > 0 && (is_token_char(Some(bytes[i - 1])) || bytes[i - 1] == b'.')
}

fn is_token_char(byte: Option<u8>) -> bool {
    matches!(byte, Some(b) if b.is_ascii_alphanumeric() || b == b'_')
}

/// Length-consuming scan of a JSON-shaped number starting at `start`.
fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    if matches!(bytes.get(i), Some(b'-') | Some(b'+')) {
        i += 1;
    }
    let digits_start = i;
    while matches!(bytes.get(i), Some(b) if b.is_ascii_digit()) {
        i += 1;
    }
    if i == digits_start {
        return start;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while matches!(bytes.get(i), Some(b) if b.is_ascii_digit()) {
            i += 1;
        }
    }
    if matches!(bytes.get(i), Some(b'e') | Some(b'E')) {
        let mut k = i + 1;
        if matches!(bytes.get(k), Some(b'-') | Some(b'+')) {
            k += 1;
        }
        if matches!(bytes.get(k), Some(b) if b.is_ascii_digit()) {
            i = k;
            while matches!(bytes.get(i), Some(b) if b.is_ascii_digit()) {
                i += 1;
            }
        }
    }
    i
}

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        out.push_str(&blank_durations(line));
        out.push('\n');
    }
    out
}

fn expected_path(row: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/protocol")
        .join(format!("{row}.txt"))
}

/// The whole transcript of one row, as it is stored: the argv that produced it,
/// the exit status, then each stream.
fn transcript(argv: &[String], status: i32, stdout: &str, stderr: &str) -> String {
    format!(
        "$ tididi {}\nexit {status}\n--- stdout\n{}--- stderr\n{}",
        argv.join(" "),
        normalize(stdout),
        normalize(stderr),
    )
}

fn check_row(row: &str, flags: &[&str], cnf: &str) {
    let mut argv: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
    if !flags.contains(&"--time-limit-ms") {
        argv.extend(["--time-limit-ms".into(), TIME_LIMIT_MS.into()]);
    }
    argv.extend(["--stats".into(), "-".into()]);
    argv.push(cnf.into());

    let output = Command::new(BINARY)
        .args(&argv)
        // The rows pass repo-relative CNF paths so the transcript carries no
        // machine path; cargo already runs integration tests here, and saying
        // so keeps the expected files valid however the test is launched.
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|e| panic!("row {row}: could not run the binary: {e}"));

    let actual = transcript(
        &argv,
        output.status.code().unwrap_or(-1),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    );

    let path = expected_path(row);
    if std::env::var_os(UPDATE_VAR).is_some() {
        std::fs::write(&path, &actual)
            .unwrap_or_else(|e| panic!("row {row}: could not write {}: {e}", path.display()));
        println!("row {row}: wrote {}", path.display());
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "row {row}: no expected transcript at {} ({e}).\n\
             Create it with:\n    {}",
            path.display(),
            regenerate()
        )
    });
    if actual == expected {
        return;
    }

    let (line_no, want, got) = first_difference(&expected, &actual);
    panic!(
        "row {row}: output differs from {}\n\
         first difference at line {line_no}:\n\
         \x20 expected: {want}\n\
         \x20   actual: {got}\n\
         If the change is intended, regenerate and read the diff:\n    {}",
        path.display(),
        regenerate()
    );
}

/// The 1-based line number of the first difference, with both sides' text.
/// `<missing>` stands for a side that has run out of lines.
fn first_difference(expected: &str, actual: &str) -> (usize, String, String) {
    let mut want = expected.lines();
    let mut got = actual.lines();
    let mut line_no = 0;
    loop {
        line_no += 1;
        match (want.next(), got.next()) {
            (None, None) => return (line_no, "<end>".into(), "<end>".into()),
            (w, g) if w == g => continue,
            (w, g) => {
                let show = |s: Option<&str>| s.map_or("<missing>".to_string(), str::to_string);
                return (line_no, show(w), show(g));
            }
        }
    }
}

/// One `#[test]` per row, so a failure names the row and cargo schedules the
/// rows across cores. Every row but one is quick; the one that spends most of
/// its time limit reaching the projected compile is `#[ignore]`d into the
/// thorough tier that `CONTRIBUTING.md` describes. A row names its own
/// `--time-limit-ms` only where the shared limit would make a clock-bounded
/// stage stop at a machine-dependent point.
macro_rules! protocol_rows {
    ($( $( #[$attr:meta] )* $row:ident : [$($flag:literal),*], $cnf:literal; )*) => {
        $(
            #[test]
            $( #[$attr] )*
            fn $row() {
                check_row(stringify!($row), &[$($flag),*], $cnf);
            }
        )*
    };
}

protocol_rows! {
    // --- the compile, the cascades and the projected stages, one row per mode
    compile_tiny:         ["--mode", "compile"], "tests/data/tiny.cnf";
    mc_all_backbone:      ["--mode", "mc"], "tests/data/cnfs/pmc/bmc/cnt06.shuffled.cnf";
    mc_s641:              ["--mode", "mc"], "tests/data/cnfs/iscas89/s641.scan.cnf";
    // SAT pruning bounds each probe pass by a share of the time limit, 200 ms
    // under the limit the other rows run at, short enough that where a pass
    // stops could move with the machine. Under an hour the share is 6 s and
    // the walk runs to its own end every time.
    mc_sat_prune:         ["--mode", "mc", "--time-limit-ms", "3600000"], "tests/data/cnfs/satlib/uf100/uf100-01000.cnf";
    // SAT pruning removes too few of the nodes it checks and stops before
    // the compile ends.
    mc_sat_prune_backs_off: ["--mode", "mc"], "tests/data/cnfs/mcc/2025/track1/mc2025_track1_025.cnf";
    wmc_tiny_sat:         ["--mode", "wmc"], "tests/data/wmc_header_tiny_sat.cnf";
    wmc_unsat:            ["--mode", "wmc"], "tests/data/wmc_unsat.cnf";
    pwmc_arjun_defined:   ["--mode", "pwmc"], "tests/data/pwmc_arjun_defined_weight.cnf";
    pwmc_projected_var:   ["--mode", "pwmc"], "tests/data/pwmc_projected_var_weight.cnf";
    pwmc_decompose:       ["--mode", "pwmc"], "tests/data/pwmc_two_components.cnf";
    pmc_tiny_show:        ["--mode", "pmc"], "tests/data/pmc_tiny_show.cnf";
    // The only rows that reach the projected compile: no earlier stage answers
    // either instance, so the run falls through to preprocessing and a
    // diagram. The weighted row spends most of its time limit before its
    // compile, which is why it is the thorough tier's share.
    pmc_compile:          ["--mode", "pmc"], "tests/data/cnfs/mcc/2026/track3/mc2026_track3_187.cnf";
    #[ignore = "~50s: the row spends most of its time limit"]
    pwmc_compile:         ["--mode", "pwmc"], "tests/data/cnfs/mcc/2023/track4/mc2023_track4_097.cnf";

    // --- the streaming entry: --mode mc with an explicit --vtree -----------
    stream_mc_s641:       ["--mode", "mc", "--vtree", "portfolio"], "tests/data/cnfs/iscas89/s641.scan.cnf";
    stream_mc_all_backbone: ["--mode", "mc", "--vtree", "portfolio"], "tests/data/cnfs/pmc/bmc/cnt06.shuffled.cnf";
    stream_mc_sat_prune:  ["--mode", "mc", "--vtree", "portfolio", "--time-limit-ms", "3600000"], "tests/data/cnfs/satlib/uf100/uf100-01000.cnf";
    stream_mc_sat_prune_backs_off: ["--mode", "mc", "--vtree", "portfolio"], "tests/data/cnfs/mcc/2025/track1/mc2025_track1_025.cnf";
    stream_mc_tiny:       ["--mode", "mc", "--vtree", "portfolio"], "tests/data/mc_header_tiny.cnf";
    stream_mc_onehot:     ["--mode", "mc", "--vtree", "portfolio"], "tests/data/onehot_free_residual.cnf";
}

/// The normalizer must blank clock readings and nothing else.
#[test]
fn normalization_blanks_only_clock_readings() {
    let stats = r#"{"model_count":"18014398509481984","preprocess_ms":14,"arjun_ms":0,"backbone_probes":37,"sat_prune_probe_us":175411}"#;
    assert_eq!(
        blank_durations(stats),
        r#"{"model_count":"18014398509481984","preprocess_ms":"<t>","arjun_ms":"<t>","backbone_probes":37,"sat_prune_probe_us":"<t>"}"#
    );

    assert_eq!(
        blank_durations("[pmc] compile: preprocessing 0ms (374c→114c), vtree 177ms, compile 99ms"),
        "[pmc] compile: preprocessing <t> (374c→114c), vtree <t>, compile <t>"
    );
    assert_eq!(
        blank_durations("[wmc] compile: counted in 138 ms"),
        "[wmc] compile: counted in <t>"
    );

    // Counts and exponent-form weights stay put.
    for input in [
        "[mc] arjun: reduced in 14ms — 81v/525c → 62v/418c, lift=2^0",
        r#"{"weighted_count":"8.587500000000007e1","weighted_count_log10":1.9338667500676072}"#,
    ] {
        let blanked = blank_durations(input);
        let expected = input.replace("14ms", "<t>");
        assert_eq!(blanked, expected, "over-blanked: {input}");
    }
}
