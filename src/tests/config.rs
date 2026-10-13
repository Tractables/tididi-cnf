use super::*;

#[test]
fn parse_preprocess_level_known_values() {
    assert_eq!(parse_preprocess_level("none"), Some(PreprocessLevel::None));
    assert_eq!(parse_preprocess_level("full"), Some(PreprocessLevel::Full));
    assert_eq!(
        parse_preprocess_level("full-eq"),
        None,
        "any other spelling, retired ones included, fails",
    );
}

#[test]
fn mode_tokens_are_vitris() {
    for mode in Mode::ALL {
        assert_eq!(Mode::parse(mode.token()), Some(mode));
        assert_eq!(vitri::cnf::Mode::parse_mode(mode.token()), Some(mode.vitri()));
    }
    assert_eq!(Mode::parse("weighted"), None);
}

#[test]
fn a_mode_parses_from_the_token_it_prints_as() {
    for mode in Mode::ALL {
        assert_eq!(mode.to_string().parse::<Mode>().ok(), Some(mode));
    }
    let refused = "weighted".parse::<Mode>().expect_err("not a mode");
    assert!(matches!(refused, crate::Error::Usage(_)));
    let message = refused.to_string();
    assert!(message.contains("\"weighted\"") && message.contains("compile, mc, wmc, pmc, pwmc"), "{message}");
}

#[test]
fn a_show_line_makes_the_declared_count_projected() {
    use vitri::cnf::Mode as Header;
    assert_eq!(Mode::from_header(None, false), None);
    assert_eq!(Mode::from_header(None, true), Some(Mode::Pmc));
    assert_eq!(Mode::from_header(Some(Header::Mc), false), Some(Mode::Mc));
    assert_eq!(Mode::from_header(Some(Header::Mc), true), Some(Mode::Pmc));
    assert_eq!(Mode::from_header(Some(Header::Wmc), false), Some(Mode::Wmc));
    assert_eq!(Mode::from_header(Some(Header::Wmc), true), Some(Mode::Pwmc));
    assert_eq!(Mode::from_header(Some(Header::Pmc), false), Some(Mode::Pmc));
    assert_eq!(Mode::from_header(Some(Header::Pwmc), false), Some(Mode::Pwmc));
}
