use super::*;
use tididi::vtree::VtreeNode;

#[test]
fn a_projecting_compile_sums_out_no_leaf_level() {
    let vtree = Vtree::from_nodes(vec![
        VtreeNode::Leaf { var: VarId(1), parent: None },
        VtreeNode::Leaf { var: VarId(2), parent: None },
        VtreeNode::Internal { left: VtreeIdx(0), right: VtreeIdx(1), parent: None },
    ], VtreeIdx(2), 2).unwrap();
    let targets = [VtreeIdx(0), VtreeIdx(1), VtreeIdx(2)];
    assert!(matches!(summable(&vtree, &targets, false), Cow::Borrowed(_)));
    assert_eq!(&*summable(&vtree, &targets, true), &[VtreeIdx(2)]);
    assert!(summable(&vtree, &targets[..2], true).is_empty());
}
