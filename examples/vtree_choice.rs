//! Compile one formula on several vtrees and compare the circuits.
//! Run with `cargo run --example vtree_choice`.

use tididi_bonsai::compile::{compile_cnf, CompileOptions};
use tididi_bonsai::parse_dimacs;
use tididi_bonsai::vtree::{build_vtree, VtreeOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Two 12-bit registers x and y that hold the same nonzero value:
    // variable i is bit i of x and variable i + 12 the same bit of y.
    let bits = 12;
    let mut dimacs = format!("p cnf {} {}\n", 2 * bits, 2 * bits + 1);
    for i in 1..=bits {
        let j = i + bits;
        dimacs += &format!("-{i} {j} 0\n{i} -{j} 0\n");
    }
    let nonzero: Vec<String> = (1..=bits).map(|i| i.to_string()).collect();
    dimacs += &format!("{} 0\n", nonzero.join(" "));
    let (formula, _header) = parse_dimacs(dimacs.as_bytes())?;

    for spec in ["linear", "balanced", "minfill-primal", "portfolio"] {
        let vtree = build_vtree(&formula, spec, &VtreeOptions::default())?;
        let diagram = vtree
            .context()
            .run(|engine| compile_cnf(engine, &formula, &vtree, CompileOptions::default()))?
            .diagram;
        println!("{spec:>14}: {:>5} nodes, {} models", diagram.node_count(), diagram.model_count()?);
    }
    Ok(())
}
