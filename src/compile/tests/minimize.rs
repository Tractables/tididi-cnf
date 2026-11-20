use super::*;

#[test]
fn recovery_skips_content_twins_even_with_an_adaptive_probe() {
    let mut probe = ContentTwinSchedule::default();
    assert!(matches!(full_reduction(false, None), ReductionPlan::Full(ContentTwinPolicy::Fresh)));
    assert!(matches!(full_reduction(false, Some(&mut probe)), ReductionPlan::Full(ContentTwinPolicy::Adaptive(_))));
    assert!(matches!(full_reduction(true, None), ReductionPlan::Full(ContentTwinPolicy::Skip)));
    assert!(matches!(full_reduction(true, Some(&mut probe)), ReductionPlan::Full(ContentTwinPolicy::Skip)));
}

/// Gate state anchored at a post-minimize size of 1000 nodes, already pruned
/// at 1000.
fn anchored() -> MinimizeGate {
    MinimizeGate {
        size_threshold: (1000.0 * MIN_SIZE_MULT) as usize,
        size_threshold_retired: 0,
        last_prune_nodes: 1000,
        last_prune_retired: 0,
        ..MinimizeGate::default()
    }
}

/// The expensive tier is due exactly when the diagram is over the
/// threshold, retired slots counted back in.
#[test]
fn expensive_tier_is_due_over_the_threshold() {
    let g = anchored();
    let threshold = g.size_threshold;
    assert_eq!(g.decide(threshold + 1, 0, None), MinimizeAction::Full);
    assert_eq!(g.decide(threshold, 0, None), MinimizeAction::PruneOnly);
    assert_eq!(g.decide(threshold - 9, 10, None), MinimizeAction::Full);
    // Over a lower threshold without growth since the last prune.
    assert_eq!(MinimizeGate { size_threshold: 900, ..anchored() }.decide(1000, 0, None), MinimizeAction::ContractOnly);
}

/// Prune re-fires on growth since the last prune, at any threshold.
#[test]
fn prune_fires_on_growth() {
    let g = anchored();
    let grown = (1000.0 * PRUNE_GROWTH_TRIGGER) as usize;
    assert!(grown < g.size_threshold);
    assert_eq!(g.decide(grown + 1, 0, None), MinimizeAction::PruneOnly);
    assert_eq!(g.decide(grown, 0, None), MinimizeAction::ContractUnlessThrottled);
}

/// Where the unnamed nodes are read, they alone decide the prune: due at
/// the share of the nodes, growth or not, and not without it.
#[test]
fn unnamed_nodes_decide_the_prune_where_read() {
    let g = anchored();
    let grown = (1000.0 * PRUNE_GROWTH_TRIGGER) as usize + 1;
    let share = (1000.0 * UNNAMED_PRUNE_FRACTION) as usize;
    assert_eq!(g.decide(1000, 0, Some(share)), MinimizeAction::PruneOnly);
    assert_eq!(g.decide(1000, 0, Some(share - 1)), MinimizeAction::ContractUnlessThrottled);
    assert_eq!(g.decide(grown, 0, Some(0)), MinimizeAction::ContractUnlessThrottled);
    assert_eq!(MinimizeGate::default().decide(10, 0, Some(0)), MinimizeAction::ContractOnly);
    assert_eq!(g.decide(g.size_threshold + 1, 0, Some(share - 1)), MinimizeAction::ContractOnly);
}
