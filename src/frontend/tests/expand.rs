use super::*;
use tididi::diagram::Literal;
use tididi::vtree::VarId;

#[test]
fn fully_resolved_constants_and_free_var_keep_count_and_public_skeleton() {
    let eng = &tididi::Engine::new();
    let expansion = VariableExpansion {
        classes: Vec::new(),
        constants: vec![Literal::pos(VarId(1)), Literal::neg(VarId(2))],
        free: vec![VarId(3)],
        num_vars: 3,
    };
    let tdd = fully_resolved_tdd(eng, &expansion).unwrap();
    assert_eq!(tdd.vtree().to_text(), Vtree::balanced(3).to_text());
    assert_eq!(tdd.model_count().unwrap(), 2u32.into());
}
