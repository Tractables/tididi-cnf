/// A cofactor whose vtree cannot be built gives up with `None`, as a compile
/// that gives up does, and the recovery reads it as one. A panic is kept for a
/// broken invariant or a refused allocation, not for input vitri refuses.
#[test]
fn a_cofactor_whose_vtree_cannot_be_built_gives_up_without_panicking() {
    let eng = &tididi::Engine::new();
    let config = crate::test_support::minimal_config();
    let run = crate::test_support::run(eng, &config);
    let no_variables = crate::test_support::formula(0, Vec::new());
    let counted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::count_cofactor(&run, &no_variables)
    }));
    assert!(matches!(counted, Ok(None)));
}
