//! Vitri's CNF items as this crate uses them, where vitri's own tests do not
//! pin the same behaviour: component detection and extraction, and the
//! duplicate-variable check in `Clause::new`.

mod common;

use vitri::cnf::Literal;

use common::make_formula;

/// Components come back as a partition of the clause indices, smallest
/// first, and a connected formula has none.
#[test]
fn components_partition_the_clauses_smallest_first() {
    let formula = make_formula(6, vec![vec![1, 2], vec![-2, 3], vec![-1, -3], vec![4, 5], vec![-5, 6]]);
    assert_eq!(formula.detect_components(), Some(vec![vec![3, 4], vec![0, 1, 2]]));
    assert_eq!(make_formula(4, vec![vec![1, 2], vec![2, 3], vec![3, 4]]).detect_components(), None);
}

/// An extracted component numbers its variables from one in the order of
/// the originals, and mapping it back gives each original clause.
#[test]
fn an_extracted_component_maps_back_to_its_clauses() {
    let formula = make_formula(6, vec![vec![1, 2], vec![-2, 3], vec![-1, -3], vec![4, 6], vec![-6, 5]]);
    for component in formula.detect_components().expect("two components") {
        let (sub, local_to_global) = formula.extract_component(&component);
        assert_eq!(sub.num_vars() as usize, local_to_global.len());
        assert!(local_to_global.is_sorted(), "{local_to_global:?}");
        let mapped_back: Vec<Vec<Literal>> = sub
            .clauses()
            .iter()
            .map(|clause| {
                clause.literals.iter().map(|l| Literal::new(local_to_global[l.var.idx()], l.positive)).collect()
            })
            .collect();
        let originals: Vec<Vec<Literal>> =
            component.iter().map(|&i| formula.clauses()[i].literals.clone()).collect();
        assert_eq!(mapped_back, originals);
    }
}

/// `Clause::new` rejects a repeated variable with debug assertions only.
#[cfg(debug_assertions)]
mod duplicate_variables {
    use vitri::cnf::{Clause, Literal, VarId};

    /// `x ∨ ¬x`.
    #[test]
    #[should_panic(expected = "Clause::new: duplicate variable")]
    fn a_clause_may_not_repeat_a_variable_across_polarities() {
        let _ = Clause::new(vec![Literal::new(VarId::from_dimacs(1), true), Literal::new(VarId::from_dimacs(1), false)]);
    }

    /// `x ∨ x`.
    #[test]
    #[should_panic(expected = "Clause::new: duplicate variable")]
    fn a_clause_may_not_repeat_a_literal() {
        let _ = Clause::new(vec![Literal::new(VarId::from_dimacs(3), true), Literal::new(VarId::from_dimacs(3), true)]);
    }
}
