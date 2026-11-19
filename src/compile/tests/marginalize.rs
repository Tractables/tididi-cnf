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

/// The cofactor step declines a child with a summed-out level
/// (`Tdd::has_marginal_level`); on a home a summed node is a region, which
/// that test sees.
#[test]
fn a_region_reads_as_a_marginal_level() {
    let eng = Engine::new();
    let home = tididi::vtree::Home::new(Vtree::balanced(4));
    let own = Vtree::projection(&home, [VarId(1), VarId(2), VarId(3)]).unwrap();
    let mut f = eng.clause(&own, [1, 2, 3]).unwrap();
    assert!(!f.has_marginal_level());
    eng.marginalize(&mut f, |v| v != VarId(3)).unwrap();
    assert!(f.vtree().bottomup().any(|t| f.vtree().node(t).is_region()));
    assert!(f.has_marginal_level());
    assert_eq!(eng.model_count(&f).unwrap(), 14u32.into());
}
