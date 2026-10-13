//! The counting problem every runner works on, over the count's domain.
//!
//! A [`Problem`] is a formula, its show set, its weights and the lift its
//! answer has to carry. The [`Domain`] says what the weights, the lift and
//! the answer are: an integer count with a power-of-two lift, or an exact
//! rational with a rational lift, carried with whether the formula has a model
//! ([`WeightedCount`]).

use num_traits::One;
use vitri::cnf::CnfFormula;

/// A weighted count, and whether the formula it counts has a model at all.
///
/// The count cannot say which: a zero weight, or positive and negative weights
/// that cancel, make the count of a satisfiable formula zero. So the path that
/// answers reports what it found — a refutation in preprocessing, an empty
/// clause, no feasible assignment, a diagram that is the false constant — and
/// the count of an unsatisfiable formula is zero without being stored.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WeightedCount<V> {
    /// The formula has no model; its count is zero.
    Unsatisfiable,
    /// The formula has a model, and this is its count, which may be zero.
    Satisfiable(V),
}

impl<V> WeightedCount<V> {
    /// The count with `f` applied — a lift, a weight factor — and the same
    /// verdict.
    pub(crate) fn map(self, f: impl FnOnce(V) -> V) -> Self {
        match self {
            WeightedCount::Unsatisfiable => WeightedCount::Unsatisfiable,
            WeightedCount::Satisfiable(value) => WeightedCount::Satisfiable(f(value)),
        }
    }
}

/// The count of two formulas over disjoint variables: the product of theirs,
/// satisfiable when both are.
impl<V: std::ops::Mul<Output = V>> std::ops::Mul for WeightedCount<V> {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        match (self, other) {
            (WeightedCount::Satisfiable(a), WeightedCount::Satisfiable(b)) => WeightedCount::Satisfiable(a * b),
            _ => WeightedCount::Unsatisfiable,
        }
    }
}

/// The value domain of a count: its answer, its weights and its lift.
pub(crate) trait Domain {
    /// The answer: an integer count, or an exact rational and whether the
    /// formula has a model, which a weighted count alone cannot say.
    type Value: PartialEq;
    /// The weights, `(negative, positive)` per variable, of a weighted count;
    /// nothing for an integer one.
    type Weights;
    /// What preprocessing multiplied the answer by: a power of two, or an
    /// exact rational.
    type Lift;

    /// The answer of an unsatisfiable problem.
    fn zero() -> Self::Value;
    /// The answer of a problem with nothing left in it: no variable and no
    /// clause.
    fn one() -> Self::Value;
    /// The lift of a problem nothing has rewritten.
    fn no_lift() -> Self::Lift;
    /// The lift of two rewrites in a row.
    fn compose(first: &Self::Lift, second: Self::Lift) -> Self::Lift;
    /// The answer carried back through a lift.
    fn lifted(value: Self::Value, lift: &Self::Lift) -> Self::Value;
    /// The weights the instance declared, over `num_vars` variables.
    fn declared_weights(table: Option<&vitri::cnf::WeightTable>, num_vars: usize) -> Self::Weights;
    /// The weights and lift one preprocessing record reports.
    fn from_record(
        record: &vitri::bundle::PreprocessRecord,
        num_vars: u32,
    ) -> (Self::Weights, Self::Lift);
}

/// Integer counting: MC and PMC.
pub(crate) struct Unweighted;

/// Weighted counting: WMC and PWMC.
pub(crate) struct Weighted;

impl Domain for Unweighted {
    type Value = num_bigint::BigUint;
    type Weights = ();
    type Lift = u32;

    fn zero() -> Self::Value {
        num_bigint::BigUint::ZERO
    }
    fn one() -> Self::Value {
        num_bigint::BigUint::from(1u32)
    }
    fn no_lift() -> u32 {
        0
    }
    fn compose(first: &u32, second: u32) -> u32 {
        first + second
    }
    fn lifted(value: Self::Value, lift: &u32) -> Self::Value {
        value << *lift
    }
    fn declared_weights(_table: Option<&vitri::cnf::WeightTable>, _num_vars: usize) {}
    fn from_record(
        record: &vitri::bundle::PreprocessRecord,
        _num_vars: u32,
    ) -> ((), u32) {
        ((), record.count_lift_pow2)
    }
}

impl Domain for Weighted {
    type Value = WeightedCount<num_rational::BigRational>;
    type Weights = Vec<(num_rational::BigRational, num_rational::BigRational)>;
    type Lift = num_rational::BigRational;

    fn zero() -> Self::Value {
        WeightedCount::Unsatisfiable
    }
    fn one() -> Self::Value {
        WeightedCount::Satisfiable(num_rational::BigRational::one())
    }
    fn no_lift() -> Self::Lift {
        num_rational::BigRational::one()
    }
    fn compose(first: &Self::Lift, second: Self::Lift) -> Self::Lift {
        first * second
    }
    /// A lift scales the count; preprocessing keeps satisfiability, so the
    /// verdict stands even where the lift is zero.
    fn lifted(value: Self::Value, lift: &Self::Lift) -> Self::Value {
        value.map(|value| value * lift)
    }
    /// The table as vitri's [`resolve`](vitri::cnf::WeightTable::resolve)
    /// reads it, weighing a literal the table leaves out as the model
    /// counting competition's format does: `1 - w` beside a partner given `w`
    /// in `(0, 1)`, and 1 when neither literal is given. With no table, every
    /// literal weighs 1.
    fn declared_weights(
        table: Option<&vitri::cnf::WeightTable>,
        num_vars: usize,
    ) -> Self::Weights {
        table.map_or_else(
            || vec![(num_rational::BigRational::one(), num_rational::BigRational::one()); num_vars],
            |table| table.resolve::<vitri::cnf::Original>(num_vars).as_pairs().to_vec(),
        )
    }
    fn from_record(
        record: &vitri::bundle::PreprocessRecord,
        num_vars: u32,
    ) -> (Self::Weights, Self::Lift) {
        // A weighted record's lift is `weight_lift` alone: vitri writes its
        // power-of-two half as 0, and prints the lift from a `BigRational`,
        // so the string always parses back.
        let lift = vitri::cnf::parse_rational_weight(&record.weight_lift)
            .expect("vitri writes weight_lift as an exact rational");
        (reduced_weights(record, num_vars), lift)
    }
}

/// vitri's complete signed reduced weight record in the compiler's
/// per-variable `(negative, positive)` convention. vitri writes the table on
/// every weighted record but a refuted one, which is read as unsatisfiable
/// before this, and lists both literals of every reduced variable, each as an
/// exact `numerator/denominator`, so the rows read through vitri's own weight
/// table cover the reduced formula.
fn reduced_weights(
    record: &vitri::bundle::PreprocessRecord,
    reduced_num_vars: u32,
) -> Vec<(num_rational::BigRational, num_rational::BigRational)> {
    let weight_record = record
        .reduced_weights
        .as_ref()
        .expect("a weighted record that is not refuted carries its reduced weights");
    let pairs: Vec<_> = weight_record
        .iter()
        .map(|row| {
            // vitri prints each row from a `BigRational`, whose denominator is
            // never zero, so the string always parses back.
            let weight = vitri::cnf::parse_rational_weight(&row.weight)
                .expect("vitri writes every reduced weight as an exact rational");
            (row.literal, weight)
        })
        .collect();
    vitri::cnf::Weights::<vitri::cnf::Reduced>::from_dimacs_pairs(&pairs, reduced_num_vars as usize)
        .as_pairs()
        .to_vec()
}

/// A show set over a problem's own formula. Every problem is the input of the
/// next vitri call, so its variables are what vitri calls the original space.
pub(crate) type ShowSet = vitri::cnf::ShowSet<vitri::cnf::Original>;

/// A vitri show set as the show set of the problem over the formula it came
/// with: the same variables, in that formula's space.
pub(crate) fn problem_show<S: vitri::cnf::Space>(show: &vitri::cnf::ShowSet<S>) -> ShowSet {
    ShowSet::from_vars(show.iter_vars())
}

/// A counting problem: the formula, its show set (`None` when every variable
/// is shown), its weights, and the lift the answer has to carry back to the
/// instance.
pub(crate) struct Problem<D: Domain> {
    pub(crate) formula: CnfFormula,
    pub(crate) show: Option<ShowSet>,
    pub(crate) weights: D::Weights,
    pub(crate) lift: D::Lift,
}

impl<D: Domain> Problem<D> {
    /// The problem as the instance declares it.
    pub(crate) fn declared(formula: CnfFormula, meta: &vitri::cnf::CnfMeta) -> Self {
        let num_vars = formula.num_vars() as usize;
        let show = meta.declared_show_vars().cloned();
        Problem {
            formula,
            show,
            weights: D::declared_weights(meta.declared_weights(), num_vars),
            lift: D::no_lift(),
        }
    }

    /// This problem rewritten by `next`, which was reduced from it: the lift
    /// composes, everything else is `next`'s.
    pub(crate) fn rewritten(self, next: Problem<D>) -> Self {
        Problem {
            lift: D::compose(&self.lift, next.lift),
            ..next
        }
    }

    /// How many variables the show set holds; none without one.
    pub(crate) fn show_len(&self) -> usize {
        self.show.as_ref().map_or(0, ShowSet::len)
    }
}

#[cfg(test)]
#[path = "tests/problem.rs"]
mod tests;
