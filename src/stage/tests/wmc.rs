use tididi::Engine;
use super::compile_weighted;
use crate::test_support as fixtures;
use tididi::test_helpers::rat;

/// A weighted compile with no vtree builds its own and counts exactly.
#[test]
fn a_compile_without_a_vtree_builds_its_own() {
    let eng = &Engine::new();
    let w = vec![
        (rat(1, 2), rat(1, 3)),
        (rat(2, 5), rat(3, 7)),
        (rat(5, 11), rat(2, 9)),
        (rat(7, 4), rat(1, 6)),
        (rat(3, 8), rat(5, 2)),
    ];
    let sub = fixtures::formula(5, vec![
        fixtures::clause(&[-3, -5]),
        fixtures::clause(&[-4, 5]),
        fixtures::clause(&[3, 4]),
        fixtures::clause(&[1]),
        fixtures::clause(&[-2]),
    ]);
    let expected = fixtures::brute_projected_weighted_count(&sub, 0..sub.num_vars() as usize, &w);

    let config = fixtures::minimal_config();
    let got = compile_weighted(&fixtures::run(eng, &config), &sub, &w, None, None)
        .expect("the compile must answer");
    let crate::problem::WeightedCount::Satisfiable(got) = got else {
        panic!("the fixture is satisfiable");
    };
    // The stage counts in the log domain, so the comparison is to a tolerance.
    let got = *got.as_log().expect("the weighted count runs in the log domain");
    let expected = tididi::diagram::SignedLog::from_rational(&expected);
    assert_eq!(got.sign, expected.sign);
    assert!(
        (got.log10_abs() - expected.log10_abs()).abs() < 1e-9,
        "the weighted count must equal brute force: {got:?} vs {expected:?}",
    );
}
