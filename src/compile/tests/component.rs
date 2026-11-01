use super::*;
use std::sync::Arc;
use tididi::Engine;
use crate::compile::Output;
use crate::test_support::dimacs;

/// A split whose second component is UNSAT answers ⊥ on the vtree the
/// grafted result would have had: the components' own vtrees, as the
/// caller gave them, chained with the free variable. That holds weighted
/// too, where the ⊥ carries an empty store.
#[test]
fn an_unsat_component_answers_zero_on_the_grafted_vtree() {
    use tididi::diagram::RationalWeights;
    use tididi::vtree::{VtreeIdx, VtreeNode};
    let eng = &Engine::new();
    // Component (x1 ∨ x2) ∧ (x2 ∨ x3) ∧ (¬x1 ∨ x3), then x4 ∧ ¬x4; x5 free.
    let formula = dimacs(5, &[&[1, 2][..], &[2, 3], &[-1, 3], &[4], &[-4]]);
    let split = [
        ComponentVtree {
            vtree: Arc::new(Vtree::balanced(3)),
            component: Component { clause_indices: vec![0, 1, 2], local_to_global: vec![VarId(1), VarId(2), VarId(3)] },
        },
        ComponentVtree {
            vtree: Arc::new(Vtree::balanced(1)),
            component: Component { clause_indices: vec![3, 4], local_to_global: vec![VarId(4)] },
        },
    ];
    let nodes = |v: &Vtree| -> Vec<VtreeNode> {
        (0..v.num_nodes() as u32).map(|i| v.node(VtreeIdx(i)).clone()).collect()
    };
    let (expected, _) = Vtree::graft_over(
        &[&*split[0].vtree, &*split[1].vtree],
        |k, local| split[k].component.local_to_global[local.idx()],
        &[VarId(5)],
        5,
    )
    .unwrap();
    let weights = RationalWeights::unit(5);
    let weighted = Output::WeightedCount { weights, arithmetic: tididi::diagram::Arithmetic::ExactRational };
    for output in [Output::Function, Output::Count, weighted] {
        let weighted = matches!(output, Output::WeightedCount { .. });
        let opts = crate::compile::CompileOptions {
            plan: crate::compile::ComponentPlan::Split(&split),
            output,
            ..crate::compile::CompileOptions::default()
        };
        let tdd = crate::compile::compile_cnf(eng, &formula, &Arc::new(Vtree::balanced(5)), opts)
            .expect("an UNSAT split is an answer")
            .diagram;
        assert!(tdd.is_zero(), "weighted = {weighted}");
        assert_eq!(tdd.weights().is_some(), weighted);
        assert_eq!(tdd.vtree().root(), expected.root());
        assert_eq!(nodes(tdd.vtree()), nodes(&expected), "weighted = {weighted}");
    }
}
