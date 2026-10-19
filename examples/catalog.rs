//! Count the configurations a product's rules allow, the hardware choices
//! among them, and what remains as a customer chooses.
//! Run with `cargo run --example catalog`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use tididi_cnf::tididi::{self, Literal, Tdd};
    use tididi_cnf::{parse_dimacs, Limits, Mode};

    // Variables 1–3 choose the processor, 4–6 the memory (8, 16 or 32 GB),
    // 7 adds a graphics card and 8 a larger battery.
    let rules = "\
p cnf 8 11
1 2 3 0
-1 -2 0
-1 -3 0
-2 -3 0
4 5 6 0
-4 -5 0
-4 -6 0
-5 -6 0
-7 5 6 0
-6 3 0
-8 -7 0
";

    let limits = Limits::default();
    let answer = tididi_cnf::count(parse_dimacs(rules.as_bytes())?, Some(Mode::Mc), &limits)?;
    println!("Valid configurations: {}", answer.count());

    let hardware = format!("{rules}c p show 1 2 3 4 5 6 0\n");
    let answer = tididi_cnf::count(parse_dimacs(hardware.as_bytes())?, Some(Mode::Pmc), &limits)?;
    println!("Processor and memory pairs: {}", answer.count());

    let answer = tididi_cnf::count(parse_dimacs(rules.as_bytes())?, Some(Mode::Compile), &limits)?;
    let rules_circuit = answer.diagram().expect("a compile keeps its circuit");
    let mut counter = rules_circuit.counter()?;
    counter.observe([Literal::try_from(7)?])?;
    println!("With a graphics card: {}", counter.model_count()?);
    counter.observe([Literal::try_from(1)?])?;
    println!("And the first processor: {}", counter.model_count()?);
    counter.observe([Literal::try_from(-7)?])?;
    println!("First processor, no graphics card: {}", counter.model_count()?);

    let extra = Tdd::clause(rules_circuit.vtree(), [7, 8])?;
    let upgraded = tididi::and(rules_circuit.clone(), extra)?;
    println!("With a graphics card or larger battery: {}", upgraded.model_count()?);
    Ok(())
}
