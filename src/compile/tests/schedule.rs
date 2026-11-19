use super::*;

/// The last scopes of the clauses `literals`, scoped as `scopes` lists.
fn last_scopes(vtree: &Vtree, literals: &[&[Literal]], scopes: &[Vec<usize>]) -> Vec<u32> {
    last_scope_positions(vtree, scopes, |ci| literals[ci].iter().map(|lit| lit.var))
}

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
    let schedule = marginalize_schedule(&vtree, &last_scopes(&vtree, &literals, &scopes));
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
    let schedule = marginalize_schedule(&vtree, &last_scopes(&vtree, &literals, &scopes));
    assert_eq!(schedule[1], vec![VtreeIdx(1)]);
    assert_eq!(schedule[3], vec![VtreeIdx(3)]);
    for (step, group) in schedule.iter().enumerate() {
        assert!(group.iter().all(|&t| vtree.topo_pos(t) <= vtree.topo_pos(VtreeIdx(step as u32))));
    }
}

/// On a home the schedule is a predicate per variable: at each step, the
/// internal nodes at or below it all of whose leaves are summable are the
/// ones the node groups free at that step or before, and the projected
/// variables done there are those whose leaf the node groups free there.
#[test]
fn the_variable_schedule_frees_what_the_node_groups_free() {
    let vtree = tree();
    let lit = |v: i32| Literal::new(VarId(v.unsigned_abs()), v > 0);
    for (clauses, scopes) in [
        (vec![vec![lit(1), lit(2)], vec![lit(-2), lit(3)]], vec![vec![], vec![], vec![], vec![0], vec![1]]),
        (vec![vec![lit(1)], vec![lit(2)], vec![lit(3)]], vec![vec![0], vec![1], vec![2], vec![], vec![]]),
        (vec![vec![lit(1), lit(3)]], vec![vec![], vec![], vec![], vec![], vec![0]]),
    ] {
        let literals: Vec<&[_]> = clauses.iter().map(Vec::as_slice).collect();
        let last = last_scopes(&vtree, &literals, &scopes);
        let groups = marginalize_schedule(&vtree, &last);
        let forget: HashSet<VarId> = [VarId(1), VarId(3)].into();
        let vars = VarSchedule::new(&vtree, &last, Some(&forget));
        let leaves = |n: VtreeIdx| {
            let mut stack = vec![n];
            let mut out = Vec::new();
            while let Some(m) = stack.pop() {
                match *vtree.node(m) {
                    VtreeNode::Leaf { var, .. } => out.push(var),
                    VtreeNode::Internal { left, right, .. } => stack.extend([left, right]),
                }
            }
            out
        };
        let mut freed = HashSet::new();
        for &t in vtree.bottomup_slice() {
            freed.extend(groups[t.idx()].iter().copied());
            assert_eq!(vars.frees(t), !groups[t.idx()].is_empty(), "{clauses:?} at {t:?}");
            let step = vars.at(&vtree, t);
            for &n in vtree.bottomup_slice() {
                if vtree.node(n).is_leaf() || n == vtree.root() || vtree.topo_pos(n) > vtree.topo_pos(t) {
                    continue;
                }
                assert_eq!(leaves(n).into_iter().all(|v| step.summable(v)), freed.contains(&n), "{clauses:?} at {t:?}, {n:?}");
            }
            let mut forgets: Vec<VarId> = groups[t.idx()]
                .iter()
                .filter_map(|&d| match *vtree.node(d) {
                    VtreeNode::Leaf { var, .. } if forget.contains(&var) => Some(var),
                    _ => None,
                })
                .collect();
            forgets.sort();
            let mut at = vars.forget_at(t).to_vec();
            at.sort();
            assert_eq!(at, forgets, "{clauses:?} at {t:?}");
        }
    }
}
