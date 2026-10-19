//! Count the distinct values a multiplier circuit outputs, as a projected
//! model count over the circuit's clauses.
//! Run with `cargo run --example circuits`.

/// A circuit's clauses, as DIMACS literals (`-v` is variable `v` negated),
/// and its last variable.
#[derive(Default)]
struct Circuit {
    clauses: Vec<Vec<i32>>,
    num_vars: u32,
}

impl Circuit {
    /// A new variable, for an input or a gate's output.
    fn var(&mut self) -> u32 {
        self.num_vars += 1;
        self.num_vars
    }

    /// A gate output equal to `a ∧ b`.
    fn and(&mut self, a: u32, b: u32) -> u32 {
        let out = self.var();
        let (o, a, b) = (out as i32, a as i32, b as i32);
        self.clauses.extend([vec![-o, a], vec![-o, b], vec![o, -a, -b]]);
        out
    }

    /// A gate output equal to `a ⊕ b`.
    fn xor(&mut self, a: u32, b: u32) -> u32 {
        let out = self.var();
        let (o, a, b) = (out as i32, a as i32, b as i32);
        self.clauses.extend([vec![-o, a, b], vec![-o, -a, -b], vec![o, -a, b], vec![o, a, -b]]);
        out
    }
}

fn main() -> Result<(), tididi_cnf::Error> {
    use tididi_cnf::{formula_from_clauses, Limits, Mode};

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
    let limits = Limits::default();
    let inputs = formula_from_clauses(num_vars, &circuit.clauses, None, &[])?;
    let answer = tididi_cnf::count(inputs, Some(Mode::Mc), &limits)?;
    println!("Input pairs: {}", answer.count());

    let products = formula_from_clauses(num_vars, &circuit.clauses, Some(&[p0, p1, p2, p3]), &[])?;
    let answer = tididi_cnf::count(products, Some(Mode::Pmc), &limits)?;
    println!("Distinct products: {}", answer.count());
    Ok(())
}
