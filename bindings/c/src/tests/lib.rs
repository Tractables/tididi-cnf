//! What a C program cannot reach on purpose: a panic stopped at the boundary,
//! and the status code of each error class. `tests/conformance.c` covers the
//! rest through the header.

use std::collections::BTreeSet;

use super::*;

#[test]
fn a_panic_comes_back_as_the_panic_code_with_its_message() {
    let result = answer(|| panic!("the invariant broke"));
    assert_eq!(result.code, TIDIDI_BONSAI_ERROR_PANIC);
    let message = result.message().expect("a panic is a failed answer");
    let message = String::from_utf8_lossy(&message.0[..message.len()]);
    assert!(message.contains("the invariant broke"), "the message should carry the panic's: {message}");
}

#[test]
fn each_error_class_has_a_code_of_its_own() {
    let reason = || "reason".to_string();
    let errors = [
        Error::Usage(reason()),
        Error::Environment(reason()),
        Error::Stage(reason()),
        Error::VtreeSpec(reason()),
        Error::Input(reason()),
        Error::Artifact(reason()),
        Error::Incomplete(reason()),
    ];
    let codes: BTreeSet<tididi_bonsai_code> = errors.iter().map(code_of).collect();
    assert_eq!(codes.len(), errors.len(), "two classes share a code: {codes:?}");
    for reserved in [TIDIDI_BONSAI_OK, TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT, TIDIDI_BONSAI_ERROR_PANIC] {
        assert!(!codes.contains(&reserved), "an error class was given the reserved code {reserved}");
    }
}
