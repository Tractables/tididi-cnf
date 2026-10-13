//! `tididi_bonsai::count`, the one-call entry: the mode it resolves from the
//! header or takes as given, the count in each task's domain, the record it
//! returns, its refusals, and its agreement with the binary, which makes the
//! same run.

mod common;

use std::sync::Mutex;

use num_bigint::BigUint;
use tididi_bonsai::{Count, Error, Limits, Mode, count, parse_dimacs};

const BINARY: &str = env!("CARGO_BIN_EXE_tididi");

/// One run at a time in this process: vitri, which the run preprocesses
/// through, is not known to be safe to enter from two threads at once.
static ONE_RUN: Mutex<()> = Mutex::new(());

fn one_run() -> std::sync::MutexGuard<'static, ()> {
    ONE_RUN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn integer(n: u32) -> Count {
    Count::Integer(BigUint::from(n))
}

/// Each environment case starts a process: changing the test process's
/// environment would race other tests.
fn count_with_environment(variable: &str, value: &str, expect_error: bool) -> std::process::Output {
    isolated_count(&[(variable, value)], if expect_error { variable } else { "count" })
}

/// Run `count_environment_child` alone in a process of its own, with no
/// `VITRI_*` setting but `env`, on `case`: `count` for a run that counts, a
/// variable's name for one refused for it, or `later` for two runs either
/// side of a new setting.
fn isolated_count(env: &[(&str, &str)], case: &str) -> std::process::Output {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", "count_environment_child", "--nocapture"]);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("VITRI_") {
            child.env_remove(name);
        }
    }
    child.envs(env.iter().copied()).env("TIDIDI_TEST_COUNT_ENVIRONMENT", case);
    let output = child.output().expect("the isolated count test runs");
    assert!(output.status.success(), "{env:?}, {case}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    output
}

#[test]
fn count_environment_child() {
    let Ok(expected) = std::env::var("TIDIDI_TEST_COUNT_ENVIRONMENT") else { return };
    let formula = || parse_dimacs("p cnf 2 1\n1 2 0\n".as_bytes()).unwrap();
    if expected == "later" {
        assert_eq!(count(formula(), Some(Mode::Mc), &Limits::default()).unwrap().count(), &integer(3));
        // Safety: this process runs this one test, on one thread.
        unsafe { std::env::set_var("VITRI_BUDGET_MS", "1") };
        let result = count(formula(), Some(Mode::Mc), &Limits::default());
        assert!(matches!(&result, Err(Error::Environment(message)) if message.contains("VITRI_BUDGET_MS")),
            "a setting made after the first run must be checked by the second, got {result:?}");
        return;
    }
    let result = count(formula(), Some(Mode::Mc), &Limits::default());
    if expected == "count" {
        assert_eq!(result.unwrap().count(), &integer(3));
    } else {
        assert!(matches!(&result, Err(Error::Environment(message)) if message.contains(&expected)),
            "expected an environment error naming {expected}, got {result:?}");
    }
}

#[test]
fn count_environment_rejects_invalid_preprocessing_controls() {
    count_with_environment("VITRI_ARJUN_SBVA", "garbage", true);
}

#[test]
fn count_environment_rejects_an_inert_vitri_budget() {
    count_with_environment("VITRI_BUDGET_MS", "1", true);
}

/// Every run reads the environment as it is when the run starts.
#[test]
fn count_environment_is_read_by_every_run() {
    isolated_count(&[], "later");
}

#[test]
fn count_environment_rejects_unused_learned_clauses() {
    count_with_environment("VITRI_ARJUN_EXPORT_LEARNED_CLAUSES", "1", true);
}

#[test]
fn count_environment_honors_sbva_policy() {
    // This formula is coloring-like. Only SBVA On arms the retry that can
    // turn SBVA off, so the diagnostic observes the policy actually in use.
    for (policy, armed) in [("on", true), ("off", false)] {
        let output = count_with_environment("VITRI_ARJUN_SBVA", policy, false);
        assert_eq!(String::from_utf8_lossy(&output.stderr).contains("no-sbva-retry: armed"), armed,
            "VITRI_ARJUN_SBVA={policy}");
    }
}

/// Without a mode, the header decides: `c t mc` is a model count, `c t pmc`
/// with its show line a projected one, `c t wmc` a weighted one in the log
/// domain and `c t pwmc` a projected weighted one, exact.
#[test]
fn the_header_names_the_task() {
    let _one = one_run();
    let limits = Limits::default();

    // 1 ∨ 2 ∨ 3: seven of the eight assignments.
    let answer = count("tests/data/mc_header_tiny.cnf", None, &limits).unwrap();
    assert_eq!((answer.mode(), answer.satisfiable()), (Mode::Mc, true));
    assert_eq!(answer.count(), &integer(7));
    assert!(answer.diagram().is_none(), "a model count keeps no diagram");

    // The header comment's count: ten distinct projections on the show set.
    let answer = count("tests/data/pmc_tiny_show.cnf", None, &limits).unwrap();
    assert_eq!((answer.mode(), answer.count()), (Mode::Pmc, &integer(10)));

    // 1 ∨ 2 ∨ 3 with 1 weighed 3/10 and 7/10 and the rest 1 both ways: the
    // eight assignments weigh (3/10 + 7/10)·2·2 = 4 together, and the one
    // that falsifies the clause, ¬1 ¬2 ¬3, weighs 7/10, so 3.3 to the log
    // domain's precision.
    let answer = count("tests/data/wmc_header_tiny_sat.cnf", None, &limits).unwrap();
    assert_eq!(answer.mode(), Mode::Wmc);
    let Count::Log(value) = answer.count() else { panic!("a weighted count is in the log domain: {:?}", answer.count()) };
    assert!((value.log10_abs() - 3.3f64.log10()).abs() < 1e-9 && value.sign == 1, "{value:?}");
    assert_eq!(answer.count().to_string(), "3.2999999999999985e0");

    // Example 4's projection with 1 weighed 0.4 and 0.6 and 2 weighed 0.3 and
    // 0.7: 0.6·0.7 + 0.6·0.3 + 0.4·0.7 = 22/25, exact.
    let answer = count("tests/data/mcc_pwmc_sat.cnf", None, &limits).unwrap();
    assert_eq!(answer.mode(), Mode::Pwmc);
    let Count::Rational(value) = answer.count() else { panic!("a projected weighted count is exact: {:?}", answer.count()) };
    assert_eq!(value, &num_rational::BigRational::new(22.into(), 25.into()));
    assert_eq!(answer.count().to_string(), "22/25");
}

/// A count converts to the nearest `f64` in every domain, and a magnitude
/// beyond its range is infinite, as a large integer's or a log-domain value's
/// can be.
#[test]
fn a_count_converts_to_the_nearest_f64() {
    use tididi::diagram::SignedLog;
    assert_eq!(integer(3).to_f64(), 3.0);
    assert_eq!(Count::Integer(BigUint::from(1u32) << 2000u32).to_f64(), f64::INFINITY);
    assert_eq!(Count::Rational(num_rational::BigRational::new((-1).into(), 4.into())).to_f64(), -0.25);
    assert_eq!(Count::Log(SignedLog::zero()).to_f64(), 0.0);
    let minus_two = Count::Log(SignedLog { ln_abs: 2f64.ln(), sign: -1 }).to_f64();
    assert!((minus_two + 2.0).abs() < 1e-12, "{minus_two}");
    assert_eq!(Count::Log(SignedLog { ln_abs: 1e4, sign: 1 }).to_f64(), f64::INFINITY);
}

/// A mode given explicitly counts a formula without a header, a formula with
/// no header and no mode compiles, and a mode the header contradicts is
/// refused as a usage error before anything runs.
#[test]
fn a_mode_given_overrides_the_default_and_must_agree_with_the_header() {
    let _one = one_run();
    let limits = Limits::default();
    // (1 ∨ 2) ∧ (¬1 ∨ 3): 1 forces 3, ¬1 forces 2, and the third is free.
    let formula = || parse_dimacs("p cnf 3 2\n1 2 0\n-1 3 0\n".as_bytes()).unwrap();

    let answer = count(formula(), Some(Mode::Mc), &limits).unwrap();
    assert_eq!((answer.mode(), answer.count()), (Mode::Mc, &integer(4)));

    let answer = count(formula(), None, &limits).unwrap();
    assert_eq!((answer.mode(), answer.count()), (Mode::Compile, &integer(4)));
    let diagram = answer.diagram().expect("a compile answers with its diagram");
    assert_eq!(diagram.model_count().unwrap(), BigUint::from(4u32));
    assert_eq!(answer.stats().tdd_nodes, Some(diagram.node_count() as u64));

    let refused = count("tests/data/mc_header_tiny.cnf", Some(Mode::Wmc), &limits);
    assert!(matches!(&refused, Err(Error::Usage(message)) if message.contains("c t mc")), "{refused:?}");
}

/// The verdict is the run's, not the count's, and the record says how the
/// run ended.
#[test]
fn the_answer_carries_the_verdict_and_the_record() {
    let _one = one_run();
    let answer = count("tests/data/mcc_mc_unsat.cnf", None, &Limits::default()).unwrap();
    assert_eq!((answer.satisfiable(), answer.count()), (false, &integer(0)));
    let stats = answer.stats();
    assert_eq!(stats.mode, Some("mc"));
    assert_eq!(stats.exit_reason, Some("solved"));
    assert_eq!(stats.satisfiable, Some(false));
    assert_eq!(stats.model_count.as_deref(), Some("0"));
    assert_eq!((stats.variables, stats.clauses), (Some(2), Some(4)));
}

/// A projected record names the show set the count was answered over and the
/// time its counting stages took, whichever stage answered: here the
/// enumeration ahead of preprocessing, and a split into two components that
/// are compiled one after the other.
#[test]
fn a_projected_record_describes_the_whole_count() {
    let _one = one_run();
    let enumerated = count("tests/data/pmc_tiny_show.cnf", None, &Limits::default()).unwrap();
    assert_eq!(enumerated.stats().show_vars, Some(4));
    assert!(enumerated.stats().compile_ms.is_some());
    let split = count("tests/data/pwmc_two_components.cnf", None, &Limits::default()).unwrap();
    assert_eq!(split.count().to_string(), "49/1");
    assert_eq!(split.stats().show_vars, Some(6), "the record kept one component's show set");
}

/// Runs on one thread share nothing: a weighted count in the log domain, then
/// a projected weighted one in exact rationals, then the first again, each
/// with its own domain and its own record.
#[test]
fn runs_on_one_thread_are_independent() {
    let _one = one_run();
    let limits = Limits::default();
    let first = count("tests/data/wmc_header_tiny_sat.cnf", None, &limits).unwrap();
    let exact = count("tests/data/mcc_pwmc_sat.cnf", None, &limits).unwrap();
    let again = count("tests/data/wmc_header_tiny_sat.cnf", None, &limits).unwrap();
    assert!(matches!(exact.count(), Count::Rational(_)), "{:?}", exact.count());
    assert_eq!(first.count(), again.count());
    assert_eq!(first.stats().weighted_count_sign, Some(1));
    assert_eq!(exact.stats().mode, Some("pwmc"));
    assert_eq!(exact.stats().weighted_count_sign, None, "a record carried a field from the run before");
}

/// A memory limit lasts as long as the run: when `count` returns, the
/// process's address-space limit is what it was before, so a program that
/// counts keeps its own.
#[test]
fn a_memory_limit_ends_with_the_run() {
    let _one = one_run();
    let address_space = || {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // Safety: `limit` is a valid `rlimit` for `getrlimit` to write into.
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut limit) }, 0);
        (limit.rlim_cur, limit.rlim_max)
    };
    let before = address_space();
    // Below the soft limit, so the run lowers it, and far above what the
    // tests running beside this one map.
    let mib = 1024 * 1024;
    let mem_limit_mb = if before.0 == libc::RLIM_INFINITY { 1 << 20 } else { before.0 / mib - 1 };
    let formula = parse_dimacs("p cnf 2 1\n1 2 0\n".as_bytes()).unwrap();
    let answer = count(formula, Some(Mode::Mc), &Limits { mem_limit_mb, ..Limits::default() }).unwrap();
    assert_eq!(answer.count(), &integer(3));
    assert_eq!(address_space(), before);
}

#[test]
fn a_file_that_cannot_be_read_is_an_input_error() {
    let _one = one_run();
    let missing = count("tests/data/no_such_instance.cnf", None, &Limits::default());
    assert!(matches!(&missing, Err(Error::Input(message)) if message.contains("no_such_instance")), "{missing:?}");
}

/// The binary and the library make one run: the same count on stdout, and
/// the same record field for field, clock readings aside.
#[test]
fn the_binary_answers_as_the_library_does() {
    let _one = one_run();
    for (cnf, plain) in [
        ("tests/data/mc_header_tiny.cnf", "model count"),
        ("tests/data/pmc_tiny_show.cnf", "projected model count"),
        ("tests/data/wmc_header_tiny_sat.cnf", "weighted count"),
        ("tests/data/mcc_pwmc_sat.cnf", "projected weighted count"),
    ] {
        let answer = count(cnf, None, &Limits::default()).unwrap();

        let record_path = common::tmp_path(&format!("count_api_{}.json", cnf.replace('/', "_")));
        // `--mem-limit-mb 0` leaves the address space as the library does.
        let output = std::process::Command::new(BINARY)
            .args(["--mem-limit-mb", "0", "--stats", record_path.to_str().unwrap(), cnf])
            .output()
            .expect("the binary runs");
        assert!(output.status.success(), "{cnf}: {}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim_end(), format!("{plain}: {}", answer.count()), "{cnf}");

        let record: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&record_path).unwrap()).unwrap();
        let _ = std::fs::remove_file(&record_path);
        let library = serde_json::to_value(answer.stats()).unwrap();
        assert_eq!(without_clocks(library), without_clocks(record), "{cnf}");
    }
}

/// A record without the fields that read the clock.
fn without_clocks(record: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(fields) = record else { panic!("a record is an object: {record}") };
    fields.into_iter().filter(|(key, _)| !key.ends_with("_ms") && !key.ends_with("_us")).collect()
}
