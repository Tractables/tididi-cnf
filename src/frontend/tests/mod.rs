use super::*;
use crate::test_support::{self as fixtures, dimacs, three_variable_formula};
use tididi::test_helpers::rat;

#[test]
fn wmc_frontend_returns_one_aligned_bundle() {
    let formula = dimacs(2, &[&[1, -2]]);
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
    assert_eq!(vtree.for_fallback(&reduced, &parent, None).root.num_leaves(), reduced.num_vars());
    assert_eq!(
        report.stages.arjun,
        Some(vitri::bundle::StageOutcome::Skipped(
            vitri::bundle::SkipReason::NotRequested,
        )),
    );
}

/// The WMC frontend runs no implied-clause pass; the fallback runs it on the
/// frontend's tree, on the run's budget, when it takes that tree.
#[test]
fn the_wmc_fallback_runs_the_implied_clause_pass_itself() {
    let clauses: Vec<[i32; 2]> = (1..8).map(|i| [-i, i + 1]).collect();
    let formula = dimacs(8, &clauses);
    let mut parent = crate::config::Config {
        preprocess_level: crate::config::PreprocessLevel::None,
        ..fixtures::minimal_config()
    };
    parent.vitri.implicates_ms = Some(10_000);
    let WmcFrontend { outcome: Outcome::Reduced(problem), vtree: Some(vtree), .. } =
        wmc_frontend(&formula, &vitri::cnf::WeightTable::default(), &parent, None)
            .expect("the WMC frontend must run")
    else {
        panic!("disabled simplify must preserve the chain");
    };
    assert!(vtree.0.implied_clauses.iter().all(Vec::is_empty), "the frontend ran the pass");
    let on = vitri::RunConfig { implicates_ms: Some(10_000), ..vitri::RunConfig::default() };
    let expected: usize = vitri::component::implied_clauses(&problem.formula, &vtree.0, &on).iter().map(Vec::len).sum();
    let built = vtree.for_fallback(&problem.formula, &parent, None);
    let found = built.implied.len()
        + built.components.iter().flatten().map(|c| c.component.implied.len()).sum::<usize>();
    assert_eq!(found, expected);
}

#[test]
fn wmc_frontend_maps_unsat_without_a_weight_table() {
    let formula = dimacs(1, &[&[1], &[-1]]);
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
    let bundle = mc_streaming_preprocess(&three_variable_formula(), &parent, None)
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
    let formula = three_variable_formula();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

    let reduced_vars =
        with_mc_frontend(&formula, &parent, Some(deadline), TreeFor::MainCompile, |attempt, _session| {
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

/// The implied clauses a frontend tree carries, its components' included.
fn implied_count(vtree: &FrontendVtree) -> usize {
    match vtree {
        FrontendVtree::Built(built) => {
            built.implied.len() + built.components.iter().flatten().map(|c| c.component.implied.len()).sum::<usize>()
        }
        FrontendVtree::FullyResolved | FrontendVtree::Refuted => 0,
    }
}

/// Only the main compile's tree gets implied clauses: a recovery's frontend,
/// such as a Shannon cofactor's, and a retry from the main frontend's session
/// find none, so a run whose main compile gave up does not pay for the pass
/// again.
#[test]
fn only_the_main_compiles_frontend_tree_gets_implied_clauses() {
    let cnf = include_bytes!("../../../tests/data/cnfs/satlib/uf100/uf100-01000.cnf");
    let (formula, _) = crate::formula::parse_dimacs(&cnf[..]).expect("the fixture parses");
    let mut parent = crate::config::Config {
        mode: crate::config::Mode::Mc,
        preprocess_level: crate::config::PreprocessLevel::Full,
        time_limit_ms: Some(3_600_000),
        ..fixtures::minimal_config()
    };
    parent.vitri.implicates_ms = Some(2_000);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3600);
    let (main, retried) = with_mc_frontend(&formula, &parent, Some(deadline), TreeFor::MainCompile, |attempt, session| {
        let budget = vitri::bundle::RetryBudget::new(deadline, std::time::Duration::from_secs(10)).expect("a budget");
        let retry = vitri::bundle::FrontendRetryConfig {
            arjun_sbva: Some(vitri::preprocess::ArjunSbva::Off),
            ..vitri::bundle::FrontendRetryConfig::default()
        };
        let retried = session.retry(budget, &retry).expect("the retry runs").expect("Arjun keeps a reduction");
        (implied_count(&attempt.vtree), implied_count(&mc_frontend_attempt(retried, &parent.vtree_time).vtree))
    })
    .expect("the frontend runs");
    assert!(main > 0, "the main compile's tree has no implied clauses");
    assert_eq!(retried, 0, "the session's retry ran the pass");
    let recovery =
        with_mc_frontend(&formula, &parent, Some(deadline), TreeFor::Recovery, |attempt, _| implied_count(&attempt.vtree))
            .expect("the frontend runs");
    assert_eq!(recovery, 0, "a recovery's frontend ran the pass");
}
