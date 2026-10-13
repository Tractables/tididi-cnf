use super::*;

#[test]
fn a_vocabulary_reads_back_each_spelling_and_lists_them_for_any_other() {
    for level in PreprocessLevel::ALL {
        assert_eq!(one_of("--preprocess", level.token(), &PreprocessLevel::ALL, PreprocessLevel::token), Ok(level));
    }
    assert_eq!(
        one_of("--preprocess", "half", &PreprocessLevel::ALL, PreprocessLevel::token),
        Err("unknown --preprocess \"half\"; valid: none, full.".to_owned()),
    );
}

#[test]
fn mode_tokens_are_vitris() {
    for mode in Mode::ALL {
        assert_eq!(vitri::cnf::Mode::parse_mode(mode.token()), Some(mode.vitri()));
    }
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
