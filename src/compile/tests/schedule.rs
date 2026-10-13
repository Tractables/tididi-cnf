use super::*;

fn tree() -> Vtree {
    Vtree::from_nodes(vec![
        VtreeNode::Leaf { var: VarId(1), parent: None },
        VtreeNode::Leaf { var: VarId(2), parent: None },
        VtreeNode::Leaf { var: VarId(3), parent: None },
        VtreeNode::Internal { left: VtreeIdx(0), right: VtreeIdx(1), parent: None },
        VtreeNode::Internal { left: VtreeIdx(3), right: VtreeIdx(2), parent: None },
    ], VtreeIdx(4), 3).unwrap()
}

#[test]
fn the_schedule_waits_for_the_last_mention_and_keeps_children_first() {
    let vtree = tree();
    let clauses = [
        vec![Literal::new(VarId(1), true), Literal::new(VarId(2), true)],
        vec![Literal::new(VarId(2), false), Literal::new(VarId(3), true)],
    ];
    let literals: Vec<&[_]> = clauses.iter().map(Vec::as_slice).collect();
    let scopes = vec![vec![], vec![], vec![], vec![0], vec![1]];
    let schedule = marginalize_schedule(&literals, &vtree, &scopes);
    assert_eq!(schedule[3], vec![VtreeIdx(0)]);
    assert_eq!(schedule[4], vec![VtreeIdx(1), VtreeIdx(2), VtreeIdx(3)]);
    assert!(!schedule.iter().flatten().any(|&t| t == vtree.root()));
    assert!(schedule.iter().all(|group| group.windows(2).all(|w| vtree.topo_pos(w[0]) < vtree.topo_pos(w[1]))));
}

#[test]
fn step_completion_only_frees_the_step_targets_after_their_last_clause() {
    let vtree = tree();
    let clauses = [
        vec![Literal::new(VarId(1), true), Literal::new(VarId(3), true)],
        vec![Literal::new(VarId(2), true)],
        vec![Literal::new(VarId(1), false)],
    ];
    let literals: Vec<&[_]> = clauses.iter().map(Vec::as_slice).collect();
    let completions = step_completions(&literals, &vtree, &[VtreeIdx(0), VtreeIdx(1), VtreeIdx(3)]);
    assert_eq!(completions, vec![vec![], vec![VtreeIdx(1)], vec![VtreeIdx(0), VtreeIdx(3)]]);
    assert!(step_completions(&[], &vtree, &[VtreeIdx(0)]).is_empty());
}

/// A node whose subtree's clauses all lie below it is scheduled at its own
/// step, where its children are joined, not at the last step that reads its
/// subtree: its level must stay structural until then.
#[test]
fn no_node_is_scheduled_before_its_own_step() {
    let vtree = tree();
    let clauses = [
        vec![Literal::new(VarId(1), true)],
        vec![Literal::new(VarId(2), true)],
        vec![Literal::new(VarId(3), true)],
    ];
    let literals: Vec<&[_]> = clauses.iter().map(Vec::as_slice).collect();
    let scopes = vec![vec![0], vec![1], vec![2], vec![], vec![]];
    let schedule = marginalize_schedule(&literals, &vtree, &scopes);
    assert_eq!(schedule[1], vec![VtreeIdx(1)]);
    assert_eq!(schedule[3], vec![VtreeIdx(3)]);
    for (step, group) in schedule.iter().enumerate() {
        assert!(group.iter().all(|&t| vtree.topo_pos(t) <= vtree.topo_pos(VtreeIdx(step as u32))));
    }
}
