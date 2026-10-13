//! The diagram library's public items as this crate uses them, where the
//! library's own tests do not pin the same behaviour: writing a `.tdd` file,
//! `lca` and `sibling`, widths and reachability, conjunction and
//! `and_clause`, and rebuilding a diagram.

mod common;
use std::collections::HashSet;
use std::sync::Arc;

use num_bigint::BigUint;
use tididi::Engine;
use tididi::OperationError;
use tididi::Tdd;
use tididi::diagram::NodeIdx;
use tididi::io::write_tdd;
use tididi_bonsai::compile::{CompileOptions, compile_cnf};
use tididi::vtree::{Vtree, VtreeIdx};

use common::{brute_force_count, make_formula};

fn compile(eng: &Engine, n: u32, clauses: Vec<Vec<i32>>, vtree: &Arc<Vtree>) -> Tdd {
    compile_cnf(eng, &make_formula(n, clauses), vtree, CompileOptions::default()).unwrap().diagram
}

#[test]
fn write_tdd_writes_one_leaf_record_per_variable() {
    let eng = &Engine::new();
    let v5 = Arc::new(Vtree::balanced(5));
    for tdd in [
        Tdd::one(&Arc::new(Vtree::balanced(3))),
        compile(eng, 5, vec![vec![1, 2], vec![-1, 3], vec![-2, 4], vec![3, 5], vec![-4, -5]], &v5),
    ] {
        let mut buf = Vec::new();
        write_tdd(&mut buf, &tdd).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let n = tdd.vtree().num_leaves();
        assert!(text.starts_with("c "), "the format reference comes first: {text}");
        assert!(text.lines().any(|line| line.starts_with(&format!("p tdd 1 {n} "))), "{text}");
        assert_eq!(text.lines().filter(|line| line.starts_with("L ")).count(), n as usize, "{text}");
        assert!(text.lines().any(|line| line.starts_with("I ")), "{text}");
    }
}

/// `lca` is the root of the smallest subtree holding both nodes and
/// `sibling` the parent's other child, checked against subtrees built from
/// `children`. The compiler scopes each clause at the `lca` of its
/// variables' leaves.
#[test]
fn lca_and_sibling_agree_with_the_children() {
    for n in [1u32, 2, 3, 4, 7] {
        for vtree in [Vtree::balanced(n), Vtree::linear(n), Vtree::random(n, 3)] {
            let nodes: Vec<VtreeIdx> = vtree.bottomup().collect();
            let mut subtree: Vec<HashSet<VtreeIdx>> = vec![HashSet::new(); vtree.num_nodes()];
            for (t, _) in vtree.leaf_bottomup() {
                subtree[t.idx()] = HashSet::from([t]);
            }
            for (t, left, right) in vtree.internal_bottomup() {
                assert_eq!((vtree.sibling(left), vtree.sibling(right)), (right, left));
                subtree[t.idx()] = &(&subtree[left.idx()] | &subtree[right.idx()]) | &HashSet::from([t]);
            }
            for &a in &nodes {
                for &b in &nodes {
                    let smallest = nodes
                        .iter()
                        .filter(|t| subtree[t.idx()].contains(&a) && subtree[t.idx()].contains(&b))
                        .min_by_key(|t| subtree[t.idx()].len())
                        .unwrap();
                    assert_eq!(vtree.lca(a, b), *smallest, "lca({a:?}, {b:?})");
                }
            }
        }
    }
}

#[test]
fn compiled_diagrams_reach_every_stored_node() {
    let eng = &Engine::new();
    for (n, clauses) in [
        (4, vec![vec![1, 2], vec![-1, 3], vec![-2, 4]]),
        (5, vec![vec![1, 2, 3], vec![-1, 4], vec![-2, 5], vec![-3, -4]]),
    ] {
        let tdd = compile(eng, n, clauses, &Arc::new(Vtree::balanced(n)));
        let reachable = tdd.reachable_nodes();
        let vtree = tdd.vtree();
        for (t, _, _) in vtree.internal_bottomup() {
            assert_eq!(tdd.reference_slot_count(t), tdd.level(t).slot_count());
            assert_eq!(reachable[t.idx()].len(), tdd.level(t).slot_count());
            assert!(reachable[t.idx()].iter().all(|&r| r), "level {}", t.0);
        }
        let widest = vtree.internal_bottomup().map(|(t, _, _)| tdd.level(t).slot_count()).max().unwrap();
        assert_eq!(tdd.max_width(), widest);
    }
}

#[test]
fn contract_reduction_keeps_the_function() {
    use tididi::reduce::ReductionPlan;

    let eng = &Engine::new();
    let vtree = Arc::new(Vtree::balanced(4));
    let mut tdd = Tdd::clause(&vtree, [1, 2]).unwrap() & Tdd::clause(&vtree, [-1, 3]).unwrap();
    let (pairs, count) = (tdd.pair_count(), tdd.model_count().unwrap());
    eng.reduce(&mut tdd, ReductionPlan::Contract).unwrap();
    assert!(tdd.pair_count() <= pairs);
    assert_eq!(tdd.model_count().unwrap(), count);
}

/// The builder copies a compiled diagram level by level unchanged, as
/// expansion does, and keeps a pair repointed during the copy: some other
/// left child for some reachable node changes the count.
#[test]
fn the_builder_copies_a_compiled_diagram_and_keeps_a_repointed_pair() {
    let eng = &Engine::new();
    let clauses = vec![vec![1, 2], vec![-2, 3], vec![3, 4], vec![-4, 5], vec![-1, -5], vec![2, -3, 4]];
    for vtree in [Vtree::balanced(5), Vtree::linear(5), Vtree::random(5, 42)].map(Arc::new) {
        let tdd = compile(eng, 5, clauses.clone(), &vtree);
        let count = tdd.model_count().unwrap();
        assert!(count > BigUint::ZERO);
        // The count of a copy with, optionally, the first pair of one node
        // given a new left child: (level, node, left).
        let copy = |repoint: Option<(VtreeIdx, usize, u32)>| {
            let mut b = Tdd::builder(eng, tdd.vtree()).unwrap();
            for (t, _, _) in tdd.vtree().internal_bottomup() {
                match repoint {
                    Some((level, node, left)) if level == t => {
                        let src = tdd.level(t);
                        for i in 0..src.nodes().len() {
                            let mut pairs = src.pairs_of_idx(i).to_vec();
                            if i == node {
                                pairs[0].left = NodeIdx(left).into();
                            }
                            b.push(eng, t, &pairs).unwrap();
                        }
                    }
                    _ => b.replace_level(eng, t, tdd.level_view(t)).unwrap(),
                }
            }
            b.finish(tdd.output()).unwrap().model_count().unwrap()
        };
        assert_eq!(copy(None), count);
        let reachable = tdd.reachable_nodes();
        let changed = tdd.vtree().internal_bottomup().any(|(t, left, _)| {
            let level = &tdd.levels()[t.idx()];
            level.nodes().iter().enumerate().any(|(node, slot)| {
                reachable[t.idx()][node] && {
                    let old = level.pairs_of(slot)[0].left.raw();
                    (0..tdd.reference_slot_count(left) as u32).any(|l| l != old && copy(Some((t, node, l))) != count)
                }
            })
        });
        assert!(changed, "no repointed pair changed the count");
    }
}

/// `and_clause` walks only the ancestors of the clause's leaves. On vtrees
/// whose node index is not topological order it must agree with conjoining
/// the compiled clause, and with brute force.
#[test]
fn and_clause_matches_conjoining_the_compiled_clause() {
    let eng = &Engine::new();
    let n = 8;
    let base = vec![vec![1, -2, 3], vec![-3, 4], vec![5, -6, 7, -8], vec![2, 6]];
    for (name, vtree) in [
        ("balanced", Arc::new(Vtree::balanced(n))),
        ("linear", Arc::new(Vtree::linear(n))),
        ("random", Arc::new(Vtree::random(n, 42))),
    ] {
        for clause in [vec![1, -8], vec![-1, 4, -5, 8], vec![2, -7], vec![3, 6]] {
            let spine = compile(eng, n, base.clone(), &vtree).and_clause(clause.as_slice()).unwrap();
            let generic = compile(eng, n, base.clone(), &vtree) & compile(eng, n, vec![clause.clone()], &vtree);
            let mut all = base.clone();
            all.push(clause.clone());
            let expected = BigUint::from(brute_force_count(n, &all));
            assert_eq!(generic.model_count().unwrap(), expected, "{name} vtree, clause {clause:?}");
            assert_eq!(spine.model_count().unwrap(), expected, "{name} vtree, clause {clause:?}");
        }
    }
}

/// Conjoining a one-variable clause into a diagram on a linear vtree: the
/// levels the clause leaves free are identities and are swapped in from the
/// other operand, and the pair filter above them once read a swapped level's
/// width as 0 and indexed out of bounds.
#[test]
fn conjunction_with_identity_levels_counts_right() {
    let eng = &Engine::new();
    let clauses = vec![vec![1, -2], vec![3, -4], vec![-1, 5], vec![-3, 6]];
    let formula = compile(eng, 6, clauses.clone(), &Arc::new(Vtree::linear(6)));
    // Two components, so the compile returns a grafted vtree; the clause
    // must be on that one.
    let unit = Tdd::clause(formula.vtree(), [1]).unwrap();
    let mut result = formula & unit;
    result.minimize().unwrap();
    let mut all = clauses;
    all.push(vec![1]);
    assert_eq!(result.model_count().unwrap(), BigUint::from(brute_force_count(6, &all)));
}

/// A memory-budget refusal in the middle of `and_clause` leaves the thread's
/// pooled scratch arrays clean, so the next unbudgeted call on the same
/// thread counts right. Release builds compile out the clean-on-entry
/// assertions, so this is the only check on that path; the oracle is brute
/// force, not another conjoin through the same scratch.
#[test]
fn and_clause_recovers_after_a_budget_refusal() {
    let eng = &Engine::new();
    let n = 8;
    let base = vec![vec![1, -2, 3], vec![-3, 4], vec![5, -6, 7, -8], vec![2, 6]];
    let vtree = Arc::new(Vtree::random(n, 7));
    let clause = [1, -8, 4];

    let acc = compile(eng, n, base.clone(), &vtree);
    let refused = {
        let _budget = eng.limits().edit(|limits| limits.with_memory_budget_bytes(Some(0)));
        eng.and_clause(acc, &clause)
    };
    assert!(matches!(refused, Err(OperationError::OverBudget)), "{refused:?}");

    let out = eng.and_clause(compile(eng, n, base.clone(), &vtree), &clause).expect("no budget is armed");
    let mut all = base;
    all.push(clause.to_vec());
    assert_eq!(out.model_count().unwrap(), BigUint::from(brute_force_count(n, &all)));
}
