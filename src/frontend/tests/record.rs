use super::*;
use crate::problem::{Unweighted, Weighted};
use crate::test_support::{self as fixtures, clause, rat, rat_ratio, tididi_formula};
use vitri::cnf::{Clause, Literal};

fn pmc_tail_config() -> vitri::RunConfig {
    super::super::request::projection_stage(
        &fixtures::minimal_config(),
        vitri::cnf::Mode::Pmc,
        None,
        None,
        vitri::config::ProjectionPolicy::Full,
    )
}

#[test]
fn preprocessing_bundle_crosses_as_one_result() {
    let original = tididi_formula();
    let config = vitri::RunConfig {
        mode: Some(vitri::cnf::Mode::Mc),
        stages: vitri::config::PreprocessStages {
            simplify: false,
            arjun: false,
        },
        retain_arjun_input: true,
        ..vitri::RunConfig::default()
    };
    let bundle = vitri::bundle::preprocess(&original, &vitri::cnf::CnfMeta::default(), &config)
        .expect("disabled stages preserve the input");

    assert_eq!(bundle.reduced.clauses(), original.clauses());
    assert_eq!(bundle.count_lift.total_pow2(), 0);
    assert_eq!(
        bundle.stages.arjun,
        Some(vitri::bundle::StageOutcome::Skipped(
            vitri::bundle::SkipReason::NotRequested
        ))
    );
    assert_eq!(bundle.record.count_lift_pow2, 0);
    let arjun_input = bundle
        .arjun_input
        .as_ref()
        .expect("the requested pre-Arjun checkpoint crosses the bridge");
    assert_eq!(arjun_input.num_vars(), original.num_vars());
    assert_eq!(arjun_input.clauses(), original.clauses());
    assert_eq!(bundle.telemetry.arjun_ms, None);
    assert!(bundle.independent_support_reduced.is_none());
    assert_eq!(
        bundle.record.reduced_to_original_dimacs.iter().collect::<Vec<_>>(),
        [Some(1), Some(2), Some(3)],
    );
}

#[test]
fn pwmc_preprocessing_crosses_folded_weights_and_exact_lift() {
    use num_bigint::BigInt;
    use num_rational::BigRational;

    // x1 ≡ ¬x2. Both variables are shown, so projected strengthening may
    // drop one only by folding its weights crosswise into the survivor.
    let formula = fixtures::formula(4, vec![
        clause(&[1, 2]),
        clause(&[-1, -2]),
        clause(&[1, 3]),
        clause(&[-3, 4]),
        clause(&[2, 4]),
        clause(&[-1, -3, -4]),
    ]);
    let rat = |n: i64| BigRational::from_integer(BigInt::from(n));
    let weights = vitri::cnf::WeightTable::from_dimacs_pairs(
        [(1, rat(2)), (-1, rat(3)), (2, rat(5)), (-2, rat(7))],
        formula.num_vars(),
    )
    .expect("the fixture weights name formula variables");
    let show = vitri::cnf::ShowSet::<vitri::cnf::Original>::from_vars(
        [vitri::cnf::VarId::from_dimacs(1), vitri::cnf::VarId::from_dimacs(2)],
    );
    let config = vitri::RunConfig {
        mode: Some(vitri::cnf::Mode::Pwmc),
        stages: vitri::config::PreprocessStages {
            arjun: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let (Outcome::Reduced(Problem { formula, show, weights, lift }), _) =
        preprocess_problem::<Weighted>(&formula, Some(&show), Some(&weights), &config)
            .expect("PWMC preprocessing must cross the bridge")
    else {
        panic!("the anti-equivalence fixture is satisfiable");
    };

    assert_eq!(
        show.as_ref().map(ShowSet::as_dimacs),
        Some(&[1][..]),
        "one shown anti-equivalent partner must survive"
    );
    assert_eq!(lift, rat(1), "the exact rational lift must cross");
    assert_eq!(weights[0].1, rat(14), "x1=true absorbs x2=false");
    assert_eq!(weights[0].0, rat(15), "x1=false absorbs x2=true");
    assert_eq!(
        weights.len(),
        formula.num_vars() as usize,
        "one pair per reduced variable",
    );
}

/// A `c p weight` line may name one literal of a variable and leave the
/// other to weigh 1. Through the projected weighted chain with its Arjun
/// reduction kept, the reduced problem's count times its lift is the
/// declared instance's count by brute force, whether Arjun eliminates the
/// weighted show variable into the lift or keeps it in the reduced table.
#[test]
fn pwmc_preprocessing_weighs_an_unnamed_literal_one() {
    use vitri::cnf::{Original, ShowSet, VarId, WeightTable};

    // The chain the PWMC stage runs: Arjun, then the projected reduction.
    let config = super::super::request::projection_stage(
        &fixtures::minimal_config(),
        vitri::cnf::Mode::Pwmc,
        None,
        Some(std::time::Duration::from_secs(60)),
        vitri::config::ProjectionPolicy::Full,
    );
    let cases = [
        ("a free show variable, its positive literal named", fixtures::formula(1, vec![]), vec![1], vec![(1, rat_ratio(3, 10))]),
        ("a free show variable, its negative literal named", fixtures::formula(1, vec![]), vec![1], vec![(-1, rat_ratio(3, 10))]),
        ("a named literal weighing 1", fixtures::formula(1, vec![]), vec![1], vec![(1, rat(1))]),
        (
            "show 1 over projected 2, 1 ∨ 2",
            fixtures::formula(2, vec![clause(&[1, 2])]),
            vec![1],
            vec![(1, rat_ratio(3, 10))],
        ),
        (
            "show 1 and 2 kept by 1 ∨ 2, projected 3 ↔ 1 ∧ 2",
            fixtures::formula(3, vec![clause(&[1, 2]), clause(&[-3, 1]), clause(&[-3, 2]), clause(&[3, -1, -2])]),
            vec![1, 2],
            vec![(1, rat_ratio(3, 10)), (2, rat_ratio(1, 4)), (-2, rat_ratio(1, 2))],
        ),
    ];
    let mut wrong = Vec::new();
    for (label, formula, show, named) in cases {
        let n = formula.num_vars();
        let declared = WeightTable::from_dimacs_pairs(named, n).expect("the case names formula variables");
        let show_idx: Vec<usize> = show.iter().map(|&v| v - 1).collect();
        let want = fixtures::brute_projected_weighted_count(
            &formula,
            show_idx,
            declared.resolve::<Original>(n as usize).as_pairs(),
        );
        let show_set = ShowSet::<Original>::from_vars(show.iter().map(|&v| VarId::from_dimacs(v as i32)));

        let (outcome, report) = preprocess_problem::<Weighted>(&formula, Some(&show_set), Some(&declared), &config)
            .expect("PWMC preprocessing must cross the bridge");
        assert_eq!(
            report.stages.arjun,
            Some(vitri::bundle::StageOutcome::Ran),
            "{label}: the case must reach a kept Arjun reduction",
        );
        let Outcome::Reduced(Problem { formula: reduced, show: reduced_show, weights, lift }) = outcome else {
            panic!("{label}: the case is satisfiable");
        };
        let reduced_show: Vec<usize> = match reduced_show {
            Some(show) => show.iter_vars().map(|v| v.idx()).collect(),
            None => (0..reduced.num_vars() as usize).collect(),
        };
        let got = fixtures::brute_projected_weighted_count(&reduced, reduced_show, &weights) * lift;
        if got != want {
            wrong.push(format!("{label}: {got}, not {want}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn wmc_bridge_calls_vitri_and_falls_back_on_an_actual_refutation() {
    let formula = fixtures::formula(1, vec![clause(&[1]), clause(&[-1])]);
    let weights = vitri::cnf::WeightTable::from_dimacs_pairs(
        [(1, rat_ratio(5, 7)), (-1, rat_ratio(2, 3))],
        formula.num_vars(),
    )
    .expect("valid");
    let config = super::super::request::arjun_stage(
        &fixtures::minimal_config(),
        vitri::cnf::Mode::Wmc,
        None,
        std::time::Duration::from_millis(1),
        vitri::config::ArjunClauseGrowth::RejectAgainst(formula.clauses().len()),
    );

    let (outcome, report) = preprocess_problem::<Weighted>(&formula, None, Some(&weights), &config)
        .expect("vitri answers a refuted formula");
    let kept = matches!(outcome, Outcome::Reduced(_))
        && matches!(report.stages.arjun, Some(vitri::bundle::StageOutcome::Ran));
    assert!(!kept, "a refuted or abandoned Arjun stage is not a kept reduction");
}

#[test]
fn wmc_outcome_exposes_exact_reduced_weights_and_lift() {
    let formula = fixtures::formula(2, vec![clause(&[1, -2])]);
    let table = vitri::cnf::WeightTable::from_dimacs_pairs(
        [(1, rat(3)), (-1, rat(2)), (2, rat(7)), (-2, rat(5))],
        formula.num_vars(),
    )
    .expect("valid WMC weights");
    let meta = vitri::cnf::CnfMeta::from_parts(
        formula.num_vars(),
        Some(vitri::cnf::Mode::Wmc),
        None,
        Some(table),
    )
    .expect("valid WMC metadata");
    let config = vitri::RunConfig {
        mode: Some(vitri::cnf::Mode::Wmc),
        stages: vitri::config::PreprocessStages {
            simplify: false,
            arjun: false,
        },
        ..vitri::RunConfig::default()
    };

    let mut bundle =
        vitri::bundle::preprocess(&formula, &meta, &config).expect("disabled WMC stages preserve input");
    bundle.record.weight_lift = "11/13".to_string();
    let rows = bundle
        .record
        .reduced_weights
        .as_mut()
        .expect("weighted record must carry every signed literal");
    rows[0].weight = "3/2".to_string();
    rows[1].weight = "2/3".to_string();

    let Outcome::Reduced(Problem {
        formula: reduced,
        weights: reduced_weights,
        lift,
        ..
    }) = outcome::<Weighted>(bundle.reduced, &bundle.record)
        .expect("a WMC record reads as a weighted problem")
    else {
        panic!("the fixture is satisfiable");
    };
    assert_eq!(reduced, formula);
    assert_eq!(lift, rat_ratio(11, 13));
    assert_eq!(
        reduced_weights,
        vec![(rat_ratio(2, 3), rat_ratio(3, 2)), (rat(5), rat(7))],
    );
}

#[test]
fn pmc_preprocessing_returns_sorted_show_and_neutral_tail_lift() {
    let formula = fixtures::formula(3, vec![clause(&[1, -3])]);
    let show = vitri::cnf::ShowSet::<vitri::cnf::Original>::from_vars(
        [vitri::cnf::VarId::from_dimacs(3), vitri::cnf::VarId::from_dimacs(1), vitri::cnf::VarId::from_dimacs(3)],
    );
    let config = pmc_tail_config();

    let (Outcome::Reduced(Problem { formula, show, lift, .. }), _) =
        preprocess_problem::<Unweighted>(&formula, Some(&show), None, &config)
            .expect("PMC preprocessing must cross the bridge")
    else {
        panic!("the fixture is satisfiable");
    };
    let show = show.expect("a projected record carries its show set");
    assert_eq!(show.as_dimacs(), [1, 3], "the bridge exposes final sorted show ids");
    assert_eq!(lift, 0, "tail-only PMC preprocessing has no count lift");
    assert!(show.iter_vars().all(|var| var.get() <= formula.num_vars()));
}

/// A refuted record crosses as `Unsat` in both projected modes, with no
/// show set or reduced weights to carry.
#[test]
fn projected_preprocessing_maps_unsat() {
    let formula = fixtures::formula(1, vec![clause(&[1]), clause(&[-1])]);
    let show = vitri::cnf::ShowSet::<vitri::cnf::Original>::from_vars([vitri::cnf::VarId::from_dimacs(1)]);
    assert!(matches!(
        preprocess_problem::<Unweighted>(&formula, Some(&show), None, &pmc_tail_config()),
        Ok((Outcome::Unsat, _)),
    ));

    let weights =
        vitri::cnf::WeightTable::from_dimacs_pairs([], formula.num_vars()).expect("empty is valid");
    let pwmc = vitri::RunConfig {
        mode: Some(vitri::cnf::Mode::Pwmc),
        stages: vitri::config::PreprocessStages {
            arjun: false,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(matches!(
        preprocess_problem::<Weighted>(&formula, Some(&show), Some(&weights), &pwmc),
        Ok((Outcome::Unsat, _)),
    ));
}

#[test]
fn pmc_outcome_uses_the_recorded_power_of_two_lift() {
    let formula = fixtures::formula(1, vec![clause(&[1])]);
    let show = vitri::cnf::ShowSet::<vitri::cnf::Original>::from_vars([vitri::cnf::VarId::from_dimacs(1)]);
    let meta = vitri::cnf::CnfMeta::from_parts(
        formula.num_vars(),
        Some(vitri::cnf::Mode::Pmc),
        Some(show),
        None,
    )
    .expect("the fixture show id is in range");
    let config = pmc_tail_config();
    let mut bundle =
        vitri::bundle::preprocess(&formula, &meta, &config).expect("PMC preprocessing must produce a bundle");
    bundle.record.count_lift_pow2 = 7;

    let Outcome::Reduced(Problem { lift, .. }) =
        outcome::<Unweighted>(bundle.reduced, &bundle.record)
            .expect("the bridge accepts its own PMC record")
    else {
        panic!("the fixture is satisfiable");
    };
    assert_eq!(lift, 7, "PMC must use record.count_lift_pow2 exactly");
}

fn identity_compile_record(num_vars: u32) -> vitri::bundle::PreprocessRecord {
    let formula = crate::test_support::formula(
        num_vars,
        vec![Clause::new(vitri::cnf::VarId::all(num_vars).map(Literal::pos).collect())],
    );
    vitri::bundle::preprocess(
        &formula,
        &vitri::cnf::CnfMeta::default(),
        &vitri::RunConfig {
            mode: Some(vitri::cnf::Mode::Compile),
            stages: vitri::config::PreprocessStages {
                simplify: false,
                ..vitri::config::PreprocessStages::default()
            },
            ..vitri::RunConfig::default()
        },
    )
    .expect("disabled compile preprocessing preserves the input")
    .record
}

#[test]
fn original_expansion_translates_every_target_kind_once() {
    use vitri::preprocess::{OriginalMap, OriginalTarget};

    let mut record = identity_compile_record(2);
    record.original_num_vars = 6;
    record.original_to_reduced_dimacs = Some(OriginalMap::from_entries(vec![
        OriginalTarget::Literal(1),
        OriginalTarget::Literal(-1),
        OriginalTarget::Constant(true),
        OriginalTarget::Constant(false),
        OriginalTarget::Free,
        OriginalTarget::Literal(2),
    ]));

    let expansion = OriginalExpansion::from_record(&record, 2);
    assert_eq!(
        expansion.classes,
        vec![
            vec![(tididi::vtree::VarId(1), true), (tididi::vtree::VarId(2), false)],
            vec![(tididi::vtree::VarId(6), true)],
        ],
    );
    assert_eq!(
        expansion.constants,
        vec![(tididi::vtree::VarId(3), true), (tididi::vtree::VarId(4), false)],
    );
    assert_eq!(expansion.free, vec![tididi::vtree::VarId(5)]);
    assert_eq!(expansion.original_num_vars, 6);
    assert!(!expansion.is_all_constants());
}

#[test]
fn original_expansion_distinguishes_all_constants_from_other_fully_resolved_maps() {
    let all_constants = OriginalExpansion {
        classes: Vec::new(),
        constants: vec![(tididi::vtree::VarId(1), true), (tididi::vtree::VarId(2), false)],
        free: Vec::new(),
        original_num_vars: 2,
    };
    assert!(all_constants.is_all_constants());

    let with_free = OriginalExpansion {
        classes: Vec::new(),
        constants: vec![(tididi::vtree::VarId(1), true)],
        free: vec![tididi::vtree::VarId(2)],
        original_num_vars: 2,
    };
    assert!(!with_free.is_all_constants());
}
