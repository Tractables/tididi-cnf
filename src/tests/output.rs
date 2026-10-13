use super::*;
use num_bigint::BigUint;

#[test]
fn an_expired_run_drops_a_late_answer_from_its_record() {
    let config = crate::test_support::minimal_config();
    let formula = crate::test_support::formula(2, vec![]);
    let mut stats = super::open_record(&config, &formula);
    let answer = Projected { value: BigUint::from(4u32), show_vars: Some(1), compile_ms: 0 };
    super::record_pmc(&mut stats, answer);
    super::close_record(&mut stats, &Err(crate::Error::Incomplete(String::new())), None, None);
    assert!(stats.model_count.is_none());
    assert!(stats.satisfiable.is_none());
    assert_eq!(stats.show_vars, Some(1));
    assert_eq!(stats.variables, Some(2));
    assert_eq!(stats.exit_reason, Some("incomplete"));
}

#[test]
fn log10_zero_is_neg_inf() {
    let v = log10_biguint(&BigUint::ZERO);
    assert!(v.is_infinite() && v.is_sign_negative());
    // Spec requires the literal token "-inf".
    assert_eq!(format!("{}", v), "-inf");
}

#[test]
fn log10_small_exact() {
    // The MCC format spec's own example: count 22 -> 1.342422...
    let v = log10_biguint(&BigUint::from(22u32));
    assert!((v - 22f64.log10()).abs() < 1e-12, "got {v}");
    assert!((log10_biguint(&BigUint::from(1u32))).abs() < 1e-12);
    assert!((log10_biguint(&BigUint::from(1000u32)) - 3.0).abs() < 1e-12);
}

#[test]
fn log10_beyond_f64_range() {
    // 2^2000 overflows f64 (max ~2^1024); the shift path must still work.
    let n = BigUint::from(1u32) << 2000u32;
    let v = log10_biguint(&n);
    let expect = 2000.0 * std::f64::consts::LOG10_2;
    assert!((v - expect).abs() < 1e-6, "got {v}, expect {expect}");
}

/// The record is a wire format: a field the run has nothing to say about is
/// ABSENT, never `null` and never `0`.
#[test]
fn an_empty_record_serializes_to_an_empty_object() {
    let stats = Stats::default();
    assert_eq!(serde_json::to_string(&stats).unwrap(), "{}");
}

/// Numbers are numbers; the count is a string because it outgrows 64 bits.
#[test]
fn the_count_is_a_decimal_string_and_the_rest_are_numbers() {
    let mut stats = Stats {
        mode: Some("mc"),
        model_count: Some((num_bigint::BigUint::from(1u32) << 70u32).to_string()),
        compile_ms: Some(12),
        ..Stats::default()
    };
    stats.sat_prune_tallies.insert("sat_prune_sat_calls", 31);
    let json = serde_json::to_string(&stats).unwrap();
    assert!(json.contains(r#""model_count":"1180591620717411303424""#), "{json}");
    assert!(json.contains(r#""compile_ms":12"#), "{json}");
    assert!(json.contains(r#""sat_prune_sat_calls":31"#), "{json}");
    assert!(!json.contains("null"), "{json}");
}
