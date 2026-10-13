use super::*;
use crate::test_support::{clause, formula};
use tididi::Engine;
use num_bigint::BigUint;

/// The attempt over `formula`: its components, one minfill vtree per
/// component, and no `2^k` lift.
fn attempt(formula: &CnfFormula) -> CompileAttempt {
    let components = crate::vtree::split_components(formula, "minfill-primal")
        .into_iter()
        .map(|component| {
            let (sub, _) = formula.extract_component(&component.clause_indices);
            let vtree = crate::vtree::build_vtree(&sub, "minfill-primal", &crate::vtree::VtreeOptions::default())
                .unwrap();
            crate::vtree::ComponentVtree { vtree, component }
        })
        .collect();
    CompileAttempt { formula: formula.clone(), components, lift_pow2: 0 }
}

/// Two identical components compile to the product of their counts, and
/// the attempt's `2^k` lift is folded into the count the compile returns.
#[test]
fn identical_components_correct_count() {
    let eng = &Engine::new();
    let config = crate::test_support::minimal_config();
    let run = crate::test_support::run(eng, &config);
    let formula = formula(4, vec![clause(&[1, 2]), clause(&[3, 4])]);
    let (count, _) = count_attempt(&run, attempt(&formula), None).expect("compile failed");
    assert_eq!(count, BigUint::from(9u32));
    let lifted = CompileAttempt { lift_pow2: 3, ..attempt(&formula) };
    let (count, _) = count_attempt(&run, lifted, None).expect("compile failed");
    assert_eq!(count, BigUint::from(72u32));
}
