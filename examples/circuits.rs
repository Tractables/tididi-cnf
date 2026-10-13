//! Count the distinct values a multiplier circuit outputs, as a projected
//! model count over the circuit's clauses.
//! Run with `cargo run --example circuits`.

use tididi_bonsai::vitri::cnf::{Clause, CnfFormula, CnfMeta, Literal, ShowSet, VarId};
use tididi_bonsai::{Limits, Mode};

/// A circuit's clauses and its last variable.
#[derive(Default)]
struct Circuit {
    clauses: Vec<Clause>,
    num_vars: u32,
}

impl Circuit {
    /// A new variable, for an input or a gate's output.
    fn var(&mut self) -> u32 {
        self.num_vars += 1;
        self.num_vars
    }

    /// A clause over DIMACS literals: `-v` is variable `v` negated.
    fn clause(&mut self, literals: &[i32]) {
        let literal = |l: i32| Literal::new(VarId::from_dimacs(l), l > 0);
        self.clauses.push(Clause::new(literals.iter().map(|&l| literal(l)).collect()));
    }

    /// A gate output equal to `a ∧ b`.
    fn and(&mut self, a: u32, b: u32) -> u32 {
        let out = self.var();
        let (o, a, b) = (out as i32, a as i32, b as i32);
        self.clause(&[-o, a]);
        self.clause(&[-o, b]);
        self.clause(&[o, -a, -b]);
        out
    }

    /// A gate output equal to `a ⊕ b`.
    fn xor(&mut self, a: u32, b: u32) -> u32 {
        let out = self.var();
        let (o, a, b) = (out as i32, a as i32, b as i32);
        self.clause(&[-o, a, b]);
        self.clause(&[-o, -a, -b]);
        self.clause(&[o, -a, b]);
        self.clause(&[o, a, -b]);
        out
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut circuit = Circuit::default();
    let [a0, a1, b0, b1] = [(); 4].map(|_| circuit.var());
    let p0 = circuit.and(a0, b0);
    let a1b0 = circuit.and(a1, b0);
    let a0b1 = circuit.and(a0, b1);
    let p1 = circuit.xor(a1b0, a0b1);
    let carry = circuit.and(a1b0, a0b1);
    let a1b1 = circuit.and(a1, b1);
    let p2 = circuit.xor(a1b1, carry);
    let p3 = circuit.and(a1b1, carry);

    let num_vars = circuit.num_vars;
    let formula = CnfFormula::new(num_vars, circuit.clauses)?;
    let limits = Limits::default();
    let answer = tididi_bonsai::count((formula.clone(), CnfMeta::default()), Some(Mode::Mc), &limits)?;
    println!("Input pairs: {}", answer.count());

    let product = ShowSet::from_dimacs_ids(&[p0, p1, p2, p3])?;
    let header = CnfMeta::from_parts(num_vars, None, Some(product), None)?;
    let answer = tididi_bonsai::count((formula, header), Some(Mode::Pmc), &limits)?;
    println!("Distinct products: {}", answer.count());
    Ok(())
}
