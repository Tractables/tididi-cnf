use super::*;
use num_bigint::BigUint;

#[test]
fn an_expired_run_drops_a_late_answer_from_its_record() {
    let config = crate::test_support::minimal_config();
    let formula = crate::test_support::formula(2, vec![]);
    let mut stats = super::open_record(&config, &formula);
    let answer = Projected { value: BigUint::from(4u32), show_vars: Some(1), compile_ms: 0 };
    super::record_pmc(&mut stats, answer);
    super::close_record(&mut stats, &Err(crate::Error::Incomplete(String::new())), &Default::default(), None);
    assert!(stats.model_count.is_none());
    assert!(stats.satisfiable.is_none());
    assert_eq!(stats.show_vars, Some(1));
    assert_eq!(stats.variables, Some(2));
    assert_eq!(stats.exit_reason, Some("incomplete"));
}

/// A run that gives up names the tree its main compile last started on, and
/// an answer that named its own tree keeps it.
#[test]
fn a_run_that_gives_up_names_the_tree_its_compile_started_on() {
    let config = crate::test_support::minimal_config();
    let formula = crate::test_support::formula(2, vec![]);
    let gave_up = || Err(crate::Error::Incomplete(String::new()));
    let mut stats = super::open_record(&config, &formula);
    super::close_record(&mut stats, &gave_up(), &Default::default(), Some(ChosenTree::of_spec("force")));
    assert_eq!(stats.vtree_candidate.as_deref(), Some("force"));
    let mut stats = super::open_record(&config, &formula);
    stats.vtree_candidate = Some("goatd-primal".to_owned());
    super::close_record(&mut stats, &gave_up(), &Default::default(), Some(ChosenTree::of_spec("force")));
    assert_eq!(stats.vtree_candidate.as_deref(), Some("goatd-primal"));
}

/// A model count's record keeps the tree its first compile started on apart
/// from the tree of the fallback that answered, and drops the latter with the
/// answer when the run gives up.
#[test]
fn a_model_count_records_its_first_tree_and_the_answering_one() {
    let config = crate::test_support::minimal_config();
    let formula = crate::test_support::formula(2, vec![]);
    let answer = || McAnswer {
        count: BigUint::from(4u32),
        peak_pairs: 0,
        preprocessing: PreprocessReport::of(&Default::default()),
        compile_ms: 0,
        vtrees: Some(CascadeVtrees {
            first: Some(ChosenTree::of_spec("force")),
            answered: Some(crate::stage::RETRY_VTREE_SPEC.to_owned()),
        }),
        attribution: Attribution { answered_by: AnsweredBy::BisectRetry, reduction: None, arjun_ms: None, cascade_ms: None },
    };
    let mut stats = super::open_record(&config, &formula);
    let verdict = super::record_mc(&mut stats, &config, answer());
    super::close_record(&mut stats, &Ok(verdict), &Default::default(), Some(ChosenTree::of_spec("force")));
    assert_eq!(stats.vtree_candidate.as_deref(), Some("force"));
    assert_eq!(stats.answered_vtree.as_deref(), Some(crate::stage::RETRY_VTREE_SPEC));
    let mut stats = super::open_record(&config, &formula);
    super::record_mc(&mut stats, &config, answer());
    super::close_record(&mut stats, &Err(crate::Error::Incomplete(String::new())), &Default::default(), None);
    assert_eq!(stats.vtree_candidate.as_deref(), Some("force"));
    assert!(stats.answered_by.is_none() && stats.answered_vtree.is_none());
}

/// The record gives the learned tree's time with the tree it names, from the
/// same portfolio build, and none with a tree no portfolio build chose.
#[test]
fn the_record_gives_the_learned_trees_time_with_the_tree_it_names() {
    let config = crate::test_support::minimal_config();
    let formula = crate::test_support::formula(2, vec![]);
    let gave_up = || Err(crate::Error::Incomplete(String::new()));
    let learned = ChosenTree { spec: "model-order".to_owned(), learned_ms: Some(7) };
    let mut stats = super::open_record(&config, &formula);
    super::close_record(&mut stats, &gave_up(), &Default::default(), Some(learned.clone()));
    assert_eq!((stats.vtree_candidate.as_deref(), stats.learned_ms), (Some("model-order"), Some(7)));
    let json = serde_json::to_value(&stats).unwrap();
    assert_eq!(json["learned_ms"], 7);
    let mut stats = super::open_record(&config, &formula);
    let answer = McAnswer {
        count: BigUint::from(4u32),
        peak_pairs: 0,
        preprocessing: PreprocessReport::of(&Default::default()),
        compile_ms: 0,
        vtrees: Some(CascadeVtrees { first: Some(learned), answered: Some("model-order".to_owned()) }),
        attribution: Attribution { answered_by: AnsweredBy::Compile, reduction: None, arjun_ms: None, cascade_ms: None },
    };
    let verdict = super::record_mc(&mut stats, &config, answer);
    super::close_record(&mut stats, &Ok(verdict), &Default::default(), None);
    assert_eq!(stats.learned_ms, Some(7));
    let mut stats = super::open_record(&config, &formula);
    let retry = ChosenTree::of_spec(crate::stage::RETRY_VTREE_SPEC);
    super::close_record(&mut stats, &gave_up(), &Default::default(), Some(retry));
    assert_eq!(stats.learned_ms, None);
    assert!(serde_json::to_value(&stats).unwrap().get("learned_ms").is_none());
    assert_eq!(ChosenTree::last_winner(&Default::default()), None);
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
/// absent, never `null` and never `0`.
#[test]
fn an_empty_record_serializes_to_an_empty_object() {
    let stats = Stats::default();
    assert_eq!(serde_json::to_string(&stats).unwrap(), "{}");
}

/// Numbers are numbers; the count is a string because it outgrows 64 bits.
#[test]
fn the_count_is_a_decimal_string_and_the_rest_are_numbers() {
    let stats = Stats {
        mode: Some("mc"),
        model_count: Some((num_bigint::BigUint::from(1u32) << 70u32).to_string()),
        compile_ms: Some(12),
        ..Stats::default()
    };
    let json = serde_json::to_string(&stats).unwrap();
    assert!(json.contains(r#""model_count":"1180591620717411303424""#), "{json}");
    assert!(json.contains(r#""compile_ms":12"#), "{json}");
    assert!(!json.contains("null"), "{json}");
}
