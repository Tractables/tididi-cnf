//! What the compiler adds on the formula side: reading DIMACS text or clauses,
//! and the two formula operations the case splits share.
//!
//! The formula itself is vitri's: [`vitri::cnf`] defines `VarId`, `Literal`,
//! `Clause`, `CnfFormula`, the header metadata and the DIMACS reader and
//! writer, and this compiler uses those types directly. [`parse_dimacs`] is
//! the crate's one way in for DIMACS text, and [`formula_from_clauses`] for
//! clauses given as DIMACS integers. The case splits read the
//! per-variable occurrence count to pick a branching variable, and condition
//! on one literal by appending it as a unit clause.

use num_rational::BigRational;
use vitri::cnf::{Clause, CnfFormula, CnfMeta, Literal, ShowSet, VarId, WeightTable};

use crate::Error;

/// Read DIMACS text into a formula and the header lines that came with it.
///
/// The formula is vitri's [`CnfFormula`](vitri::cnf::CnfFormula), and the
/// [`CnfMeta`](vitri::cnf::CnfMeta) holds the `c t` task line, the `c p show`
/// set and the `c p weight` table. The pair converts into an
/// [`Input`](crate::Input) for [`count`](crate::count), and the formula is
/// what [`compile_cnf`](crate::compile::compile_cnf) compiles.
///
/// Every CNF this crate reads, the binary's instance included, arrives through
/// here, and the reading itself is [`vitri::cnf::CnfFormula::from_dimacs`]: the
/// line kinds accepted, the ids refused and what each header line means are
/// decided in one place.
///
/// ```
/// let text = "c t mc\np cnf 2 1\n1 -2 0\n";
/// let (formula, _header) = tididi_bonsai::parse_dimacs(text.as_bytes())?;
/// assert_eq!((formula.num_vars(), formula.clauses().len()), (2, 1));
/// # Ok::<(), tididi_bonsai::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::Input`](crate::Error::Input), with vitri's sentence naming the
/// line and what is wrong with it, for text that is not DIMACS: a malformed
/// problem line, a non-integer token in clause data, a weight or show token
/// that names no rational or no variable, or any id above the `p cnf` declared
/// variable count.
pub fn parse_dimacs<R: std::io::BufRead>(
    reader: R,
) -> Result<(CnfFormula, CnfMeta), Error> {
    CnfFormula::from_dimacs(reader).map_err(|error| Error::Input(error.to_string()))
}

/// Build a formula from clauses of DIMACS integers, with the header lines a
/// projected or weighted count reads.
///
/// The formula has `num_vars` variables, and each clause is its literals
/// without the closing `0`: `3` is variable 3 and `-3` its negation. `show`
/// is the `c p show` set, the variables a projected count projects onto, and
/// `None` declares none. `weights` are the `c p weight` lines as
/// `(literal, weight)` pairs, read as the competition's format reads its lines:
/// a literal they leave out weighs `1 - w` when the other literal of its
/// variable is given a `w` strictly between 0 and 1, and 1 when neither is
/// given, and a literal given twice takes its last weight. The pair is what [`parse_dimacs`]
/// returns for the same formula written as DIMACS text, so it goes to
/// [`count`](crate::count) or [`compile_cnf`](crate::compile::compile_cnf)
/// the same way, and the run is the one that text makes.
///
/// ```
/// use num_rational::BigRational;
/// use tididi_bonsai::{count, formula_from_clauses, Count, Limits, Mode};
///
/// // (x1 ∨ x2) ∧ (¬x1 ∨ x3) ∧ (¬x2 ∨ ¬x3), projected onto x1 and x2, where
/// // x1 weighs 1/4, ¬x1 weighs 3/4, and x2 weighs 1/2, so ¬x2 weighs 1/2.
/// let clauses = [vec![1, 2], vec![-1, 3], vec![-2, -3]];
/// let show: &[u32] = &[1, 2];
/// let quarters = |n: i32| BigRational::new(n.into(), 4.into());
/// let weights = [(1, quarters(1)), (-1, quarters(3)), (2, quarters(2))];
/// let formula = formula_from_clauses(3, &clauses, Some(show), &weights)?;
/// let answer = count(formula, Some(Mode::Pwmc), &Limits::default())?;
/// // x1 ¬x2 weighs 1/4 · 1/2, and ¬x1 x2 weighs 3/4 · 1/2.
/// assert_eq!(answer.count(), &Count::Rational(BigRational::new(1.into(), 2.into())));
/// # Ok::<(), tididi_bonsai::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::Input`](crate::Error::Input) for a clause literal, shown variable
/// or weight literal that names no variable, such as `0`, or a variable past
/// `num_vars`, for a clause that names a variable twice, and for a literal
/// given alone at a weight outside `(0, 1)`, which leaves its partner none. The DIMACS reader
/// merges a repeated literal and drops a clause holding both literals of a
/// variable; clauses given as data are taken as they are, so they hold each
/// variable once.
pub fn formula_from_clauses<C: AsRef<[i32]>>(
    num_vars: u32,
    clauses: impl IntoIterator<Item = C>,
    show: Option<&[u32]>,
    weights: &[(i32, BigRational)],
) -> Result<(CnfFormula, CnfMeta), Error> {
    let input = |error: vitri::VitriError| Error::Input(error.to_string());
    let clauses = clauses
        .into_iter()
        .zip(1..)
        .map(|(clause, n)| clause_from_dimacs(clause.as_ref(), n))
        .collect::<Result<Vec<_>, _>>()?;
    let formula = CnfFormula::new(num_vars, clauses).map_err(input)?;
    let show = show.map(ShowSet::from_dimacs_ids).transpose().map_err(input)?;
    // No pairs declare no weights, as a file with no `c p weight` line does.
    let weights = (!weights.is_empty())
        .then(|| WeightTable::from_dimacs_pairs(weights.iter().cloned(), num_vars))
        .transpose()
        .map_err(input)?;
    let meta = CnfMeta::from_parts(num_vars, None, show, weights).map_err(input)?;
    Ok((formula, meta))
}

/// Clause `n` of [`formula_from_clauses`], its literals in the order the
/// DIMACS reader leaves them: by variable.
fn clause_from_dimacs(literals: &[i32], n: usize) -> Result<Clause, Error> {
    let mut clause = literals
        .iter()
        .map(|&literal| {
            let var = VarId::try_from_dimacs(literal).ok_or_else(|| {
                Error::Input(format!("clause {n} holds {literal}, which names no variable"))
            })?;
            Ok(Literal::new(var, literal > 0))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    clause.sort_unstable_by_key(|literal| literal.var);
    if let Some(pair) = clause.windows(2).find(|pair| pair[0].var == pair[1].var) {
        return Err(Error::Input(format!(
            "clause {n} names variable {} twice; a clause holds each variable once",
            pair[0].var.get()
        )));
    }
    Ok(Clause::new(clause))
}

/// How many literals of each variable of `formula` sit in clauses of at least
/// `min_len` literals, indexed by `VarId::idx`.
pub(crate) fn occurrence_counts(formula: &CnfFormula, min_len: usize) -> Vec<usize> {
    let mut occ = vec![0; formula.num_vars() as usize];
    for clause in formula.clauses().iter().filter(|c| c.literals.len() >= min_len) {
        for lit in &clause.literals {
            occ[lit.var.idx()] += 1;
        }
    }
    occ
}

/// The variable, among those `eligible` accepts by index, with the most
/// literals in clauses of at least `min_len` literals, the lowest index on a
/// tie. `None` when no eligible variable occurs in such a clause.
pub(crate) fn most_frequent_var(
    formula: &CnfFormula,
    min_len: usize,
    eligible: impl Fn(usize) -> bool,
) -> Option<VarId> {
    let mut best: Option<(usize, usize)> = None;
    for (idx, n) in occurrence_counts(formula, min_len).into_iter().enumerate() {
        if n > 0 && best.is_none_or(|(_, b)| n > b) && eligible(idx) {
            best = Some((idx, n));
        }
    }
    best.map(|(idx, _)| VarId::from_idx(idx))
}

/// `formula` with the unit clause `lit` appended. No propagation, so variable
/// ids and `num_vars` do not move.
pub(crate) fn with_unit(formula: &CnfFormula, lit: Literal) -> CnfFormula {
    let mut clauses = formula.clone().into_clauses();
    clauses.push(Clause::new(vec![lit]));
    CnfFormula::new(formula.num_vars(), clauses)
        .expect("a unit over the formula's own variables stays in its space")
}

#[cfg(test)]
#[path = "tests/formula.rs"]
mod tests;
