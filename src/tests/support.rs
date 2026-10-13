//! Formulas, oracles and a configuration the crate's unit tests share.

use vitri::cnf::{Clause, CnfFormula, Literal, VarId};

use crate::config::{Config, Mode, PreprocessLevel};

/// A test formula over `num_vars` variables, which the fixture is expected
/// to stay within.
pub(crate) fn formula(num_vars: u32, clauses: Vec<Clause>) -> CnfFormula {
    CnfFormula::new(num_vars, clauses).expect("a test formula names only variables in its space")
}

/// A literal over the variable with zero-based index `v`.
pub(crate) fn lit(v: u32, pos: bool) -> Literal {
    Literal::new(VarId::from_idx(v as usize), pos)
}

/// The projected model count of `formula` onto the variables `show`
/// (zero-based indices, each inside the formula), by enumerating every
/// assignment: the number of distinct show projections of its models. A
/// show variable no clause mentions takes both values and doubles it.
pub(crate) fn brute_projected_count(
    formula: &CnfFormula,
    show: impl IntoIterator<Item = usize>,
) -> num_bigint::BigUint {
    let show: Vec<usize> = show.into_iter().collect();
    num_bigint::BigUint::from(brute_projections(formula, &show).len())
}

/// The projected weighted count of `formula` onto `show` under `weights`,
/// `(negative, positive)` per variable, by enumerating every assignment:
/// each distinct show projection of its models weighs the product of its
/// show literals' weights, and a projected variable's weight is never
/// read.
pub(crate) fn brute_projected_weighted_count(
    formula: &CnfFormula,
    show: impl IntoIterator<Item = usize>,
    weights: &[(num_rational::BigRational, num_rational::BigRational)],
) -> num_rational::BigRational {
    let show: Vec<usize> = show.into_iter().collect();
    brute_projections(formula, &show)
        .iter()
        .map(|values| {
            show.iter()
                .zip(values)
                .map(|(&v, &value)| if value { weights[v].1.clone() } else { weights[v].0.clone() })
                .product::<num_rational::BigRational>()
        })
        .sum()
}

/// The distinct show projections of `formula`'s models, each the show
/// variables' values in `show`'s order.
fn brute_projections(formula: &CnfFormula, show: &[usize]) -> std::collections::HashSet<Vec<bool>> {
    let n = formula.num_vars() as usize;
    assert!(n <= 20, "the brute-force oracle is for tiny formulas");
    assert!(show.iter().all(|&v| v < n), "a show variable outside the formula");
    let mut projections = std::collections::HashSet::new();
    for assignment in 0u32..(1 << n) {
        let value = |v: usize| (assignment >> v) & 1 == 1;
        if formula.clauses().iter().all(|c| c.literals.iter().any(|l| value(l.var.idx()) == l.positive)) {
            projections.insert(show.iter().map(|&v| value(v)).collect::<Vec<bool>>());
        }
    }
    projections
}

/// A clause from DIMACS literals.
pub(crate) fn clause(dimacs: &[i32]) -> Clause {
    Clause::new(dimacs.iter().map(|&l| Literal::new(VarId::from_dimacs(l), l > 0)).collect())
}

/// `formula` with one more clause, for a fixture built from another.
pub(crate) fn with_clause(formula: CnfFormula, clause: Clause) -> CnfFormula {
    let num_vars = formula.num_vars();
    let mut clauses = formula.into_clauses();
    clauses.push(clause);
    self::formula(num_vars, clauses)
}

/// `(x1 ∨ ¬x2) ∧ (x2 ∨ x3)`: a small satisfiable formula, for tests that
/// need some formula to run on.
pub(crate) fn three_variable_formula() -> CnfFormula {
    formula(3, vec![clause(&[1, -2]), clause(&[2, 3])])
}

/// Minimal Config for unit tests that only exercise a couple of fields.
/// Every field is set to an inert default; tests override just what they
/// need via struct-update syntax. Every field is named, so a new field
/// forces a compile error here (single place to update).
pub(crate) fn minimal_config() -> Config {
    Config {
        vtree: None,
        portfolio_history: vitri::decompose::PortfolioBuildHistory::default(),
        vtree_time: crate::budget::VtreeTime::default(),
        source_structure_profile: None,
        preprocess_level: PreprocessLevel::None,
        mode: Mode::Compile,
        time_limit_ms: None,
        sat_prune: true,
        vitri: vitri::RunConfig::default(),
    }
}

/// A run on `eng` under `config`, with no wall and no SAT-pruning pass, as a
/// stage sees it.
pub(crate) fn run<'a>(eng: &'a tididi::Engine, config: &'a Config) -> crate::stage::Run<'a> {
    crate::stage::Run { eng, config, deadline: None, compile: Default::default() }
}

/// A SAT pruning context for `formula`, under the limits of a run with no
/// time limit.
pub(crate) fn sat_prune_ctx(formula: &CnfFormula) -> crate::compile::sat_prune::SatPruneCtx {
    crate::compile::sat_prune::SatPrune::default()
        .context(formula, false)
        .expect("CaDiCaL builds a solver")
}
