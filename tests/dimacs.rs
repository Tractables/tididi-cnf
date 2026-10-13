//! Reading DIMACS through `tididi_bonsai::parse_dimacs`: the clauses, the
//! declared track, show set and weights, and the input it refuses.

use tididi_bonsai::{Error, parse_dimacs};

/// The declared variable count and the clauses, as DIMACS literals.
fn read(input: &str) -> (u32, Vec<Vec<i32>>) {
    let formula = parse_dimacs(input.as_bytes()).unwrap_or_else(|e| panic!("{input:?}: {e}")).0;
    let clauses = formula
        .clauses()
        .iter()
        .map(|clause| clause.literals.iter().map(|literal| literal.to_dimacs()).collect())
        .collect();
    (formula.num_vars(), clauses)
}

#[test]
fn clauses_read_back_exactly() {
    let ten_weighted = (1..=10)
        .map(|v| if v % 2 == 0 { format!("w\t{v}\t-1\n") } else { format!("w\t{v}\t0.3\n") })
        .collect::<String>();
    let ten_weighted = format!("p cnf 10 3\n{ten_weighted}1 2 0\n-3 4 0\n5 -6 0\n");
    for (input, expected) in [
        ("c a comment\nc another\np cnf 2 1\n1 2 0\n", (2, vec![vec![1, 2]])),
        ("\n\np cnf 2 1\n\n1 2 0\n\n", (2, vec![vec![1, 2]])),
        ("p cnf 2 1\n1 2", (2, vec![vec![1, 2]])),
        ("p cnf 3 2\n1 2 0 -1 3 0\n", (3, vec![vec![1, 2], vec![-1, 3]])),
        ("p cnf 3 1\n-1 -2 -3 0\n", (3, vec![vec![-1, -2, -3]])),
        // `w` weight lines are skipped; the integer weight -1 is not a literal.
        ("p cnf 3 2\nw\t1\t0.5\nw\t2\t-1\nw\t3\t0.1\n1 -2 0\n2 3 0\n", (3, vec![vec![1, -2], vec![2, 3]])),
        (ten_weighted.as_str(), (10, vec![vec![1, 2], vec![-3, 4], vec![5, -6]])),
        // A `0` with no literals before it is the empty clause, so the file is
        // unsatisfiable. Skipping it once turned an unsatisfiable corpus file
        // (`LGSynth89/des_mince.cnf`) into a nonzero count.
        ("p cnf 2 2\n1 2 0\n0\n", (2, vec![vec![1, 2], vec![]])),
        ("p cnf 2 1\n1 2 0 0\n", (2, vec![vec![1, 2], vec![]])),
    ] {
        assert_eq!(read(input), expected, "{input:?}");
    }
}

/// Input that is not DIMACS is the input's error. The wording is vitri's.
#[test]
fn malformed_headers_are_refused() {
    for input in ["p notcnf 2 1\n1 2 0\n", "1 2 0\n"] {
        let result = parse_dimacs(input.as_bytes());
        assert!(matches!(result, Err(Error::Input(_))), "{input:?}: {:?}", result.err());
    }
}

/// Every id a file names is checked against the declared count, show
/// variables and weight literals included, and the refusal names it.
#[test]
fn a_show_variable_above_the_declared_count_is_refused() {
    let result = parse_dimacs(&b"c t pmc\np cnf 3 1\nc p show 1 9 0\n1 2 0\n"[..]);
    assert!(matches!(&result, Err(Error::Input(message)) if message.contains('9')), "{:?}", result.err());
}

/// The declared track is the `c t` line, not the resolved mode, which reads
/// `Mc` for a file without one too.
#[test]
fn the_declared_track_reports_the_c_t_line_not_the_resolved_mode() {
    use vitri::cnf::Mode;

    let (_, projected) = parse_dimacs(&b"c t pmc\np cnf 6 1\nc p show 2 3 0\n1 -2 0\n"[..]).unwrap();
    assert_eq!(projected.declared_track(), Some(Mode::Pmc));

    let (_, plain) = parse_dimacs(&b"c t mc\np cnf 2 1\n1 2 0\n"[..]).unwrap();
    assert_eq!(plain.mode(), Mode::Mc);
    assert_eq!(plain.declared_track(), Some(Mode::Mc), "`c t mc` is still a declared track");

    let (_, bare) = parse_dimacs(&b"c comment\np cnf 3 2\n1 -2 0\n2 3 0\n"[..]).unwrap();
    assert_eq!(bare.mode(), Mode::Mc);
    assert_eq!(bare.declared_track(), None, "no `c t` line means no declared track");
}

/// The show set reads back ascending and deduplicated across every `c p show`
/// line; the show-enumeration estimate walks it in that order.
#[test]
fn the_show_set_reads_back_ascending_and_deduplicated() {
    use vitri::cnf::VarId;

    let (_, meta) = parse_dimacs(&b"p cnf 4 1\nc p show 3 1 0\nc p show 1 2 0\n1 2 3 4 0\n"[..]).unwrap();
    assert_eq!(
        meta.declared_show_vars().map(|show| show.iter_vars().collect::<Vec<_>>()),
        Some(vec![VarId::from_dimacs(1), VarId::from_dimacs(2), VarId::from_dimacs(3)]),
        "show 3 and 1 then 1 and 2, sorted and deduplicated",
    );

    let (_, empty) = parse_dimacs(&b"p cnf 2 1\nc p show 0\n1 2 0\n"[..]).unwrap();
    assert_eq!(empty.declared_show_vars().map(|show| show.len()), Some(0), "`c p show 0` declares a set");

    let (_, absent) = parse_dimacs(&b"p cnf 2 1\n1 2 0\n"[..]).unwrap();
    assert!(absent.declared_show_vars().is_none(), "no line at all is not an empty set");
}

/// `c p weight` values in the spellings the weighted tracks use read back as
/// exact rationals.
#[test]
fn declared_weights_read_back_as_exact_rationals() {
    use num_bigint::BigInt;
    use num_rational::BigRational;
    use vitri::cnf::Mode;

    let r = |n: i64, d: i64| BigRational::new(BigInt::from(n), BigInt::from(d));
    let (_, meta) = parse_dimacs(
        &b"c t wmc\np cnf 2 1\nc p weight 1 0.7 0\nc p weight -1 3/10 0\n           c p weight 2 9.5367431640625E-7 0\n1 2 0\n"[..],
    )
    .unwrap();
    assert_eq!(meta.mode(), Mode::Wmc);
    let resolved = meta.declared_weights().expect("weights declared").resolve::<vitri::cnf::Original>(2);
    let resolved = resolved.as_pairs();
    assert_eq!(resolved[0], (r(3, 10), r(7, 10)));
    assert_eq!(
        resolved[1].1,
        BigRational::new(BigInt::from(95367431640625i64), BigInt::from(10i64).pow(20)),
        "scientific notation must stay exact, not round through f64",
    );
}
