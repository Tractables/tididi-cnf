use super::*;

#[test]
fn recovery_skips_content_twins_even_with_an_adaptive_probe() {
    let mut probe = ContentTwinSchedule::default();
    assert!(matches!(full_reduction(false, None), ReductionPlan::Full(ContentTwinPolicy::Fresh)));
    assert!(matches!(full_reduction(false, Some(&mut probe)), ReductionPlan::Full(ContentTwinPolicy::Adaptive(_))));
    assert!(matches!(full_reduction(true, None), ReductionPlan::Full(ContentTwinPolicy::Skip)));
    assert!(matches!(full_reduction(true, Some(&mut probe)), ReductionPlan::Full(ContentTwinPolicy::Skip)));
}

/// Gate state anchored at a post-minimize size of 1000 nodes and 1000 pairs,
/// already pruned there.
fn anchored() -> MinimizeGate {
    MinimizeGate {
        size_threshold: (1000.0 * MIN_SIZE_MULT) as usize,
        size_threshold_retired: 0,
        throttle: ContractThrottle { pairs_floor: Some(1000), ..ContractThrottle::default() },
        ..MinimizeGate::default()
    }
}

/// The fewest pairs that count as grown since [`anchored`]'s prune.
fn grown() -> usize {
    (1000.0 * PRUNE_GROWTH_TRIGGER) as usize + 1
}

/// The expensive tier is due exactly when the diagram's nodes are over the
/// threshold, retired slots counted back in.
#[test]
fn expensive_tier_is_due_over_the_threshold() {
    let g = anchored();
    let threshold = g.size_threshold;
    assert_eq!(g.decide(threshold + 1, grown(), 0, None), MinimizeAction::Full);
    assert_eq!(g.decide(threshold, grown(), 0, None), MinimizeAction::PruneOnly);
    assert_eq!(g.decide(threshold - 9, grown(), 10, None), MinimizeAction::Full);
    // Over a lower threshold without growth since the last prune.
    assert_eq!(MinimizeGate { size_threshold: 900, ..anchored() }.decide(1000, 1000, 0, None), MinimizeAction::ContractOnly);
}

/// Prune re-fires on growth of the pairs since the last prune, at any
/// threshold; nodes do not count, nor do retired marginal slots.
#[test]
fn prune_fires_on_growth_of_pairs() {
    let g = anchored();
    assert_eq!(g.decide(1000, grown(), 0, None), MinimizeAction::PruneOnly);
    assert_eq!(g.decide(1000, grown() - 1, 0, None), MinimizeAction::ContractUnlessThrottled);
    assert_eq!(g.decide(g.size_threshold, 1000, 0, None), MinimizeAction::ContractUnlessThrottled);
    assert_eq!(g.decide(1000, 1000, 50, None), MinimizeAction::ContractUnlessThrottled);
    assert_eq!(MinimizeGate::default().decide(10, 10, 0, None), MinimizeAction::ContractOnly);
}

/// A diagram's first merge sets its floor, a smaller result lowers it, and
/// growth is judged from there; another diagram's floor is its own.
#[test]
fn the_pairs_floor_is_the_fewest_a_diagram_held() {
    let eng = Engine::new();
    let vtree = std::sync::Arc::new(tididi::vtree::Vtree::balanced(6));
    let mut f = eng.clause(&vtree, [1, -4]).unwrap();
    let pairs = f.pair_count();
    let mut g = anchored();
    assert!(pairs < 1000 && f.node_count() < g.size_threshold);
    g.adaptive_minimize(&eng, &mut f, false, false).unwrap();
    assert_eq!(g.throttle.pairs_floor, Some(pairs));
    let grown = (pairs as f64 * PRUNE_GROWTH_TRIGGER) as usize + 1;
    assert_eq!(g.decide(10, grown, 0, None), MinimizeAction::PruneOnly);
    let first = g.swap_throttle(ContractThrottle::default());
    assert_eq!(first.pairs_floor, Some(pairs));
    assert_eq!(g.decide(10, grown, 0, None), MinimizeAction::ContractUnlessThrottled);
    let now = f.pair_count();
    g.adaptive_minimize(&eng, &mut f, false, false).unwrap();
    assert_eq!(g.throttle.pairs_floor, Some(now));
}

/// Where the unnamed nodes are read, a prune is due at their share of the
/// nodes, growth or not, and below it only with some and growth; with none,
/// never.
#[test]
fn unnamed_nodes_decide_the_prune_where_read() {
    let g = anchored();
    let share = (1000.0 * UNNAMED_PRUNE_FRACTION) as usize;
    assert_eq!(g.decide(1000, 1000, 0, Some(share)), MinimizeAction::PruneOnly);
    assert_eq!(g.decide(1000, 1000, 0, Some(share - 1)), MinimizeAction::ContractUnlessThrottled);
    assert_eq!(g.decide(1000, grown(), 0, Some(1)), MinimizeAction::PruneOnly);
    assert_eq!(g.decide(1000, grown(), 0, Some(0)), MinimizeAction::ContractUnlessThrottled);
    assert_eq!(MinimizeGate::default().decide(10, 10, 0, Some(0)), MinimizeAction::ContractOnly);
    assert_eq!(MinimizeGate { size_threshold: 900, ..anchored() }.decide(1000, 1000, 0, Some(share - 1)), MinimizeAction::ContractOnly);
    assert_eq!(g.decide(g.size_threshold + 1, grown(), 0, Some(share - 1)), MinimizeAction::Full);
}
