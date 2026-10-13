use super::*;

#[test]
fn expansion_propagates_cancellation_after_its_class_rebuild() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tididi::limits::{StopCallback, StopDecision};
    use tididi::test_helpers::assert_canonical;

    for with_constant in [false, true] {
        let eng = Engine::new();
        let vtree = Arc::new(Vtree::leaf(VarId(1)));
        let input = Tdd::one(&vtree);
        assert_canonical(&input);
        let expansion = OriginalExpansion {
            classes: vec![vec![(VarId(1), true)]],
            constants: if with_constant { vec![(VarId(2), true)] } else { vec![] },
            free: if with_constant { vec![VarId(3)] } else { vec![VarId(2), VarId(3)] },
            original_num_vars: 3,
        };

        // Observe the class rebuild's polls, then cancel on the next
        // engine operation: a constant literal or the final graft.
        let polls = Arc::new(AtomicUsize::new(0));
        let seen = polls.clone();
        {
            let _scope = eng.limits().edit(|s| s.with_stop_callback(Some(StopCallback::new(move |_, _| {
                seen.fetch_add(1, Ordering::Relaxed);
                StopDecision::Continue
            }))));
            let class_vtree = expand_class_vtree(input.vtree(), &expansion);
            let classes = expand_tdd_classes(&eng, &input, &class_vtree, &expansion).unwrap();
            assert_canonical(&classes);
        }
        let class_polls = polls.swap(0, Ordering::Relaxed);
        let seen = polls.clone();
        {
            let _scope = eng.limits().edit(|s| s.with_stop_callback(Some(StopCallback::new(move |_, _| {
                if seen.fetch_add(1, Ordering::Relaxed) >= class_polls {
                    StopDecision::Stop
                } else {
                    StopDecision::Continue
                }
            }))));
            assert!(matches!(expand_to_originals(&eng, &input, &expansion), Err(OperationError::Stopped)));
        }
        let retry = expand_to_originals(&eng, &input, &expansion).unwrap();
        assert_canonical(&retry);
        assert_eq!(retry.model_count().unwrap(), num_bigint::BigUint::from(if with_constant { 4u32 } else { 8u32 }));
    }
}

fn one_leaf_tdd(eng: &Engine, local: NodeIdx) -> Tdd {
    let vtree = Arc::new(Vtree::balanced(1));
    Tdd::builder(eng, &vtree)
        .unwrap()
        .finish(TddNodeId { vtree: vtree.root(), local })
        .expect("a one-leaf vtree's root level is the leaf's three implicit nodes")
}

#[test]
fn fully_resolved_constants_and_free_var_keep_count_and_public_skeleton() {
    let eng = &tididi::Engine::new();
    let expansion = OriginalExpansion {
        classes: Vec::new(),
        constants: vec![(VarId(1), true), (VarId(2), false)],
        free: vec![VarId(3)],
        original_num_vars: 3,
    };
    let tdd = fully_resolved_tdd(eng, &expansion).unwrap();
    assert_eq!(
        tdd.vtree().to_text(),
        Vtree::balanced(3).to_text(),
    );
    assert_eq!(tdd.model_count().unwrap(), 2u32.into());
}

#[test]
fn negative_singleton_swaps_pos_and_neg_but_not_one() {
    let eng = &tididi::Engine::new();
    let expansion = OriginalExpansion {
        classes: vec![vec![(VarId(1), false)]],
        constants: Vec::new(),
        free: Vec::new(),
        original_num_vars: 1,
    };
    assert_eq!(
        expand_to_originals(eng, &one_leaf_tdd(eng, POS_LEAF_IDX), &expansion).unwrap()
            .output()
            .local,
        NEG_LEAF_IDX,
    );
    assert_eq!(
        expand_to_originals(eng, &one_leaf_tdd(eng, NEG_LEAF_IDX), &expansion).unwrap()
            .output()
            .local,
        POS_LEAF_IDX,
    );
    assert_eq!(
        expand_to_originals(eng, &one_leaf_tdd(eng, ONE_LEAF_IDX), &expansion).unwrap()
            .output()
            .local,
        ONE_LEAF_IDX,
    );
}

#[test]
fn aliases_constants_and_free_leaf_reconstruct_the_full_count() {
    let eng = &tididi::Engine::new();
    let expansion = OriginalExpansion {
        classes: vec![vec![(VarId(1), true), (VarId(2), false), (VarId(3), true)]],
        constants: vec![(VarId(4), true), (VarId(5), false)],
        free: vec![VarId(6)],
        original_num_vars: 6,
    };
    for (reduced, expected) in [
        (POS_LEAF_IDX, 2u32),
        (NEG_LEAF_IDX, 2u32),
        (ONE_LEAF_IDX, 4u32),
    ] {
        let expanded = expand_to_originals(eng, &one_leaf_tdd(eng, reduced), &expansion).unwrap();
        let mut originals: Vec<u32> = expanded.vtree().leaf_bottomup().map(|(_, var)| var.0).collect();
        originals.sort_unstable();
        assert_eq!(originals, (1..=6).collect::<Vec<_>>());
        assert_eq!(expanded.model_count().unwrap(), expected.into());
    }
}

#[test]
fn class_expansion_places_each_class_original_once_and_keeps_classes_adjacent() {
    let reduced = Vtree::balanced(2);
    let expansion = OriginalExpansion {
        classes: vec![
            vec![(VarId(1), true), (VarId(2), false)],
            vec![(VarId(6), true)],
        ],
        constants: vec![(VarId(3), true), (VarId(4), false)],
        free: vec![VarId(5)],
        original_num_vars: 6,
    };

    let class_vtree = expand_class_vtree(&reduced, &expansion);
    let mut originals: Vec<u32> = class_vtree.leaf_bottomup().map(|(_, var)| var.0).collect();
    originals.sort_unstable();
    assert_eq!(originals, [1, 2, 6]);

    let leaf0 = class_vtree.leaf_of(VarId(1)).expect("the vtree carries this variable");
    let leaf1 = class_vtree.leaf_of(VarId(2)).expect("the vtree carries this variable");
    let (left, right) = class_vtree.children(class_vtree.lca(leaf0, leaf1));
    assert!(left == leaf0 || left == leaf1);
    assert!(right == leaf0 || right == leaf1);
}
