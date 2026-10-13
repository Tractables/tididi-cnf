use num_rational::BigRational;
use vitri::cnf::{CnfFormula, CnfMeta, VarId, WeightTable};

use super::{formula_from_clauses, most_frequent_var, parse_dimacs};
use crate::test_support::{clause, formula};
use crate::Error;

/// The pick is the most frequent eligible variable, the lowest index on a
/// tie, counted only in clauses of at least the given length.
#[test]
fn the_most_frequent_eligible_variable_wins_and_a_tie_goes_to_the_lowest() {
    let v = VarId::from_dimacs;
    let any = |_: usize| true;
    // x1 is in 3 clauses, x3 in 2, x2 in 1.
    let f = formula(3, vec![clause(&[1]), clause(&[1, 3]), clause(&[-1, 2]), clause(&[-3])]);
    assert_eq!(most_frequent_var(&f, 0, any), Some(v(1)));
    assert_eq!(most_frequent_var(&f, 0, |i| i != 0), Some(v(3)));
    assert_eq!(most_frequent_var(&f, 0, |i| i == 1), Some(v(2)));
    assert_eq!(most_frequent_var(&f, 0, |_| false), None);
    assert_eq!(most_frequent_var(&formula(0, vec![]), 0, any), None);
    // In clauses of two or more literals x1 occurs twice, x2 and x3 once
    // each, so without x1 their tie goes to x2.
    assert_eq!(most_frequent_var(&f, 2, any), Some(v(1)));
    assert_eq!(most_frequent_var(&f, 2, |i| i != 0), Some(v(2)));
    // A variable only in unit clauses is no candidate at length two.
    let g = formula(4, vec![clause(&[4]), clause(&[-4])]);
    assert_eq!(most_frequent_var(&g, 0, any), Some(v(4)));
    assert_eq!(most_frequent_var(&g, 2, any), None);
    // A three-way tie goes to the lowest index.
    let h = formula(3, vec![clause(&[3, 2]), clause(&[-1, -3]), clause(&[2, 1])]);
    assert_eq!(most_frequent_var(&h, 0, any), Some(v(1)));
    assert_eq!(most_frequent_var(&h, 0, |i| i != 0), Some(v(2)));
}

fn rational(numer: i32, denom: i32) -> BigRational {
    BigRational::new(numer.into(), denom.into())
}

/// Clauses given as data, in any literal order, build the pair the DIMACS
/// reader returns for the same formula as text, header lines included.
#[test]
fn clauses_build_what_the_same_text_reads() {
    let text = "p cnf 3 4\nc p show 1 2 0\nc p weight 1 1/4 0\nc p weight -2 3 0\n1 2 0\n-1 3 0\n-2 -3 0\n0\n";
    let (read, read_meta) = parse_dimacs(text.as_bytes()).unwrap();
    let clauses: [&[i32]; 4] = [&[2, 1], &[-1, 3], &[-3, -2], &[]];
    let show: &[u32] = &[2, 1];
    let weights = [(1, rational(1, 4)), (-2, rational(3, 1))];
    let (built, built_meta) = formula_from_clauses(3, clauses, Some(show), &weights).unwrap();
    assert_eq!(built, read);
    assert_eq!(built_meta.declared_track(), read_meta.declared_track());
    assert_eq!(built_meta.declared_show_vars(), read_meta.declared_show_vars());
    assert_eq!(
        built_meta.declared_weights().map(WeightTable::to_literal_pairs),
        read_meta.declared_weights().map(WeightTable::to_literal_pairs),
    );
}

/// No weight pairs declare no weights, as a file without `c p weight` lines
/// does, so an unweighted count does not refuse the formula.
#[test]
fn no_weight_pairs_declare_no_weights() {
    let (_, meta) = formula_from_clauses(1, [[1]], None, &[]).unwrap();
    assert!(meta.declared_weights().is_none());
    assert!(meta.declared_show_vars().is_none());
}

/// `result` must be an input error whose message mentions `needle`.
fn assert_refused(result: Result<(CnfFormula, CnfMeta), Error>, needle: &str) {
    match result {
        Err(Error::Input(message)) => {
            assert!(message.contains(needle), "{message:?} does not mention {needle:?}");
        }
        other => panic!("expected an input error mentioning {needle:?}, got {other:?}"),
    }
}

/// Each id that names no variable of the formula, and each clause that names
/// a variable twice, is an input error saying which.
#[test]
fn clauses_naming_no_variable_or_one_twice_are_refused() {
    assert_refused(formula_from_clauses(3, [vec![1], vec![2, 0]], None, &[]), "clause 2 holds 0");
    assert_refused(formula_from_clauses(3, [[i32::MIN]], None, &[]), "clause 1 holds -2147483648");
    assert_refused(formula_from_clauses(3, [[1, -4]], None, &[]), "variable 4");
    assert_refused(formula_from_clauses(3, [[1, -1]], None, &[]), "clause 1 names variable 1 twice");
    assert_refused(formula_from_clauses(3, [[3, 2, 3]], None, &[]), "clause 1 names variable 3 twice");
    assert_refused(formula_from_clauses(3, [[1]], Some(&[0][..]), &[]), "0 is not a variable");
    assert_refused(formula_from_clauses(3, [[1]], Some(&[4][..]), &[]), "show variable 4");
    assert_refused(formula_from_clauses(3, [[1]], None, &[(-4, rational(1, 2))]), "weight literal -4");
    assert_refused(formula_from_clauses(3, [[1]], None, &[(0, rational(1, 2))]), "weight literal 0");
}
