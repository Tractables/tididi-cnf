use super::*;

/// The cofactor step declines a child with a summed-out level
/// (`Tdd::has_marginal_level`); on a home a summed node is a region, which
/// that test sees.
#[test]
fn a_region_reads_as_a_marginal_level() {
    let eng = Engine::new();
    let home = tididi::vtree::Home::new(Vtree::balanced(4));
    let mut f = eng.clause(&home, [1, 2, 3]).unwrap();
    assert!(!f.has_marginal_level());
    eng.marginalize(&mut f, |v| v != VarId(3)).unwrap();
    assert!(f.vtree().bottomup().any(|t| f.vtree().node(t).is_region()));
    assert!(f.has_marginal_level());
    assert_eq!(eng.model_count(&f).unwrap(), 14u32.into());
}
