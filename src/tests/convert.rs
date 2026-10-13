use super::*;

/// A vtree crosses this boundary on every build, so the two crates'
/// exact constructions have to agree on the tree they name.
#[test]
fn vitri_and_tididi_build_the_same_exact_trees() {
    assert_eq!(
        vtree_from_vitri(&vitri::vtree::Vtree::balanced(5)).to_text(),
        Vtree::balanced(5).to_text(),
    );

    let order = [
        tididi::vtree::VarId(3),
        tididi::vtree::VarId(1),
        tididi::vtree::VarId(4),
        tididi::vtree::VarId(2),
    ];
    let vitri_order: Vec<_> = order.iter().map(|v| vitri::vtree::VarId::new(v.0).unwrap()).collect();
    assert_eq!(
        vtree_from_vitri(&vitri::vtree::Vtree::linear_from_order(&vitri_order)).to_text(),
        Vtree::linear_from_order(&order).unwrap().to_text(),
    );
}
