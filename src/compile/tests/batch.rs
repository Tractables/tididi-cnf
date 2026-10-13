use super::*;
use tididi::vtree::VarId;

/// Uncapped through an accumulator width of `UNCAPPED_ACC_WIDTH`, the given
/// limit above.
#[test]
fn batch_width_cap_is_a_step_function() {
    assert_eq!(batch_width_cap(UNCAPPED_ACC_WIDTH, BATCH_WIDTH_LIMIT), usize::MAX);
    assert_eq!(batch_width_cap(UNCAPPED_ACC_WIDTH + 1, BATCH_WIDTH_LIMIT), BATCH_WIDTH_LIMIT);
    assert_eq!(batch_width_cap(1_000_000, BATCH_WIDTH_GUARD), BATCH_WIDTH_GUARD);
}

/// The floor after a non-absorbing merge; after an absorbing one,
/// `BATCH_GROWTH` times the merged batch, counted from at least the floor.
#[test]
fn next_batch_limit_grows_only_after_absorbing_merges() {
    assert_eq!(next_batch_limit(500, false), BATCH_SIZE);
    assert_eq!(next_batch_limit(3, true), BATCH_SIZE * BATCH_GROWTH.0 / BATCH_GROWTH.1);
    assert_eq!(next_batch_limit(100, true), 100 * BATCH_GROWTH.0 / BATCH_GROWTH.1);
    assert!(next_batch_limit(1, true) > BATCH_SIZE);
}

#[test]
fn batch_spine_is_the_union_of_ancestor_paths() {
    let vtree = Vtree::balanced(4);
    let mut marked = vec![false; vtree.num_nodes()];
    let mut touched = Vec::new();
    for variable in [1, 2, 1, 4] {
        mark_clause_levels(&vtree, &[tididi::Literal::pos(VarId(variable))], &mut marked, &mut touched);
    }
    let mut expected = std::collections::BTreeSet::new();
    for variable in [1, 2, 4] {
        let mut node = Some(vtree.leaf_of(VarId(variable)).unwrap());
        while let Some(level) = node {
            expected.insert(level);
            node = vtree.node(level).parent();
        }
    }
    assert_eq!(touched.len(), expected.len());
    assert_eq!(touched.into_iter().collect::<std::collections::BTreeSet<_>>(), expected);
    for (index, value) in marked.into_iter().enumerate() {
        assert_eq!(value, expected.contains(&VtreeIdx(index as u32)));
    }
}
