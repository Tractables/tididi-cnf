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

/// Over every level of a pruned diagram, the reached width is the library's
/// width.
#[test]
fn reached_on_every_level_of_a_pruned_diagram_is_its_width() {
    let eng = &Engine::new();
    let vtree = Home::new(tididi::vtree::Vtree::balanced(6));
    let diagram = eng.clause(&vtree, [Literal::pos(VarId(1)), Literal::neg(VarId(4))]).unwrap();
    let mut diagram =
        eng.and_clause(diagram, &[Literal::pos(VarId(2)), Literal::pos(VarId(5)), Literal::neg(VarId(6))]).unwrap();
    eng.reduce(&mut diagram, tididi::reduce::ReductionPlan::Prune).unwrap();
    let mut reach = ReachScratch::default();
    assert_eq!(reach.reached_width(&diagram), diagram.max_width());
    assert_eq!(reach.reached_width(&diagram), diagram.max_width());
}
