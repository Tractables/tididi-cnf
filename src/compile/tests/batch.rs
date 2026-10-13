use super::*;
use tididi::vtree::VarId;

/// Uncapped through an accumulator width of `UNCAPPED_ACC_WIDTH`,
/// `BATCH_WIDTH_LIMIT` above.
#[test]
fn batch_width_cap_is_a_step_function() {
    assert_eq!(batch_width_cap(UNCAPPED_ACC_WIDTH), usize::MAX);
    assert_eq!(batch_width_cap(UNCAPPED_ACC_WIDTH + 1), BATCH_WIDTH_LIMIT);
    assert_eq!(batch_width_cap(1_000_000), BATCH_WIDTH_LIMIT);
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
