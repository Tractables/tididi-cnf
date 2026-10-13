# Distinct outputs of a circuit

How many different products can a 2-bit multiplier output? Its two inputs
take 16 value pairs, but some pairs share a product: 1 × 2 = 2 × 1, and
anything times zero is zero. Counting the distinct products is a
**projected** model count: of the circuit's encoding, count the assignments to
the output bits that some input produces.

Run the complete program with `cargo run --example circuits`.

## Encode gates as clauses

A circuit becomes a CNF formula by giving every gate output a variable and
writing clauses that force it to equal the gate's function of its inputs.
This is the Tseitin encoding. The clauses are kept as DIMACS integers, `-v`
for variable `v` negated, rather than written out as text. An AND gate takes
three clauses; the XOR gate in the complete program takes four.

```rust,ignore,{class=tested-example}
/// A gate output equal to `a ∧ b`.
fn and(&mut self, a: u32, b: u32) -> u32 {
    let out = self.var();
    let (o, a, b) = (out as i32, a as i32, b as i32);
    self.clauses.extend([vec![-o, a], vec![-o, b], vec![o, -a, -b]]);
    out
}
```

## Build the multiplier

The inputs are `a = 2·a1 + a0` and `b = 2·b1 + b0`. The product's bits come
from the four partial products `ai ∧ bj` and one carry:

```rust,ignore,{class=tested-example}
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
```

## Count inputs, then products

Each input pair fixes every gate, so the formula's model count is the number
of input pairs. [`formula_from_clauses`](crate::formula_from_clauses) builds
the formula from the clauses, and [`count`](crate::count) counts it as the
task [`Mode::Mc`](crate::Mode::Mc):

```rust,ignore,{class=tested-example}
let num_vars = circuit.num_vars;
let limits = Limits::default();
let inputs = formula_from_clauses(num_vars, &circuit.clauses, None, &[])?;
let answer = tididi_bonsai::count(inputs, Some(Mode::Mc), &limits)?;
println!("Input pairs: {}", answer.count());
```

Output:

```text
Input pairs: 16
```

A show set, the `c p show` line of a DIMACS file, names the product's bits.
Projecting onto them counts each product once, however many input pairs give
it:

```rust,ignore,{class=tested-example}
let products = formula_from_clauses(num_vars, &circuit.clauses, Some(&[p0, p1, p2, p3]), &[])?;
let answer = tididi_bonsai::count(products, Some(Mode::Pmc), &limits)?;
println!("Distinct products: {}", answer.count());
```

Output:

```text
Distinct products: 7
```

The products of numbers from 0 to 3 are 0, 1, 2, 3, 4, 6 and 9.

The [complete program](https://github.com/Tractables/tididi-bonsai/blob/v0.1.0/examples/circuits.rs)
has the XOR gate.
