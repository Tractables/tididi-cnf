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
    for floor in [BATCH_SIZE, BATCH_LARGE_SIZE] {
        assert_eq!(next_batch_limit(500, false, floor), floor);
        assert_eq!(next_batch_limit(3, true, floor), floor * BATCH_GROWTH.0 / BATCH_GROWTH.1);
        assert_eq!(next_batch_limit(1000, true, floor), 1000 * BATCH_GROWTH.0 / BATCH_GROWTH.1);
        assert!(next_batch_limit(1, true, floor) > floor);
    }
}

/// A fold with no accumulator yet starts at the small floor.
#[test]
fn batch_floor_without_accumulator_is_small() {
    assert_eq!(batch_floor(None), BatchFloor { size: BATCH_SIZE, width_limit: BATCH_WIDTH_LIMIT });
    const { assert!(BATCH_LARGE_SIZE > BATCH_SIZE && BATCH_LARGE_WIDTH_LIMIT > BATCH_WIDTH_LIMIT) };
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

/// Over every level, the spine count is the library's whole-diagram pair
/// count, which the sum restates level by level.
#[test]
fn batch_spine_pairs_over_every_level_is_the_pair_count() {
    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(6));
    let diagram = eng.clause(&vtree, [Literal::pos(VarId(1)), Literal::neg(VarId(4))]).unwrap();
    let diagram =
        eng.and_clause(diagram, &[Literal::pos(VarId(2)), Literal::pos(VarId(5)), Literal::neg(VarId(6))]).unwrap();
    let every: Vec<VtreeIdx> = (0..vtree.num_nodes() as u32).map(VtreeIdx).collect();
    assert_eq!(batch_spine_pairs(&diagram, &every), diagram.pair_count());
}
