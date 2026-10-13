use super::*;
use crate::test_support::{self as fixtures, clause, tididi_formula};
use tididi::test_helpers::rat;

#[test]
fn wmc_frontend_returns_one_aligned_bundle() {
    let formula = fixtures::formula(2, vec![clause(&[1, -2])]);
    let weights = vec![
        (rat(2, 3), rat(5, 7)),
        (rat(11, 13), rat(17, 19)),
    ];
    let table = vitri::cnf::WeightTable::from_dimacs_pairs(
        [
            (1, rat(5, 7)),
            (-1, rat(2, 3)),
            (2, rat(17, 19)),
            (-2, rat(11, 13)),
        ],
        formula.num_vars(),
    )
    .expect("valid");
    let parent = crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::None,
        ..fixtures::minimal_config()
    };

    let WmcFrontend {
        outcome: Outcome::Reduced(crate::problem::Problem {
            formula: reduced,
            weights: reduced_weights,
            lift,
            ..
        }),
        vtree: Some(vtree),
        report,
    } = wmc_frontend(&formula, &table, &parent, None)
        .expect("the WMC frontend must run")
    else {
        panic!("disabled simplify must preserve this nontrivial formula");
    };

    assert_eq!(reduced.clauses(), formula.clauses());
    assert_eq!(reduced_weights, weights);
    assert_eq!(lift, rat(1, 1));
    assert_eq!(vtree.root.num_leaves(), reduced.num_vars());
    assert_eq!(
        report.stages.arjun,
        Some(vitri::bundle::StageOutcome::Skipped(
            vitri::bundle::SkipReason::NotRequested,
        )),
    );
}

#[test]
fn wmc_frontend_maps_unsat_without_a_weight_table() {
    let formula = fixtures::formula(1, vec![clause(&[1]), clause(&[-1])]);
    let table = vitri::cnf::WeightTable::from_dimacs_pairs(
        [(1, rat(5, 7)), (-1, rat(2, 3))],
        formula.num_vars(),
    )
    .expect("valid");
    let parent = crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::Full,
        ..fixtures::minimal_config()
    };

    match wmc_frontend(&formula, &table, &parent, None) {
        Ok(WmcFrontend { outcome: Outcome::Unsat, vtree: None, .. }) => {}
        Ok(WmcFrontend { outcome: Outcome::Unsat, vtree: Some(_), .. }) => panic!("nothing is built for a refuted record"),
        Ok(WmcFrontend { outcome: Outcome::Reduced(problem), .. }) => panic!(
            "the contradictory units survived as {}v/{}c",
            problem.formula.num_vars(),
            problem.formula.clauses().len(),
        ),
        Err(error) => panic!("the WMC frontend rejected the fixture: {error}"),
    }
}

#[test]
fn streaming_mc_preprocess_crosses_without_arjun_outputs() {
    // With the stage off, vitri reports Arjun as not requested.
    let parent = crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::Full,
        ..fixtures::minimal_config()
    };
    let bundle = mc_streaming_preprocess(&tididi_formula(), &parent, None)
        .expect("the simplify-only MC bundle crosses the boundary");
    assert_eq!(
        bundle.stages.arjun,
        Some(vitri::bundle::StageOutcome::Skipped(
            vitri::bundle::SkipReason::NotRequested,
        )),
    );
}

#[test]
fn plain_mc_frontend_scopes_one_conversion_and_the_prepared_session() {
    let parent = crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::None,
        time_limit_ms: Some(5_000),
        ..fixtures::minimal_config()
    };
    let formula = tididi_formula();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

    let reduced_vars =
        with_mc_frontend(&formula, &parent, Some(deadline), |attempt, _session| {
            let bundle = &attempt.preprocessed;
            assert_eq!(bundle.record.mode, vitri::cnf::Mode::Mc);
            assert_eq!(bundle.count_lift.total_pow2(), bundle.record.count_lift_pow2);
            assert!(
                bundle.arjun_input.is_some(),
                "the compiler requested vitri's pre-Arjun checkpoint",
            );
            assert!(bundle.telemetry.total_ms <= 5_000);
            assert!(
                bundle
                    .independent_support_reduced
                    .as_ref()
                    .is_none_or(|support| support
                        .iter_vars()
                        .all(|var| (var.idx() as u32) < bundle.reduced.num_vars()))
            );
            match &attempt.vtree {
                FrontendVtree::Built(built) => {
                    assert_eq!(built.root.num_leaves(), bundle.reduced.num_vars());
                }
                FrontendVtree::FullyResolved => {
                    assert_eq!(bundle.reduced.num_vars(), 0);
                }
                FrontendVtree::Refuted => {
                    assert!(bundle.record.unsat);
                }
            }
            bundle.reduced.num_vars()
        })
        .expect("the whole-run frontend must reach its continuation");

    assert!(reduced_vars <= formula.num_vars());
}
