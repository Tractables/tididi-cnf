//! Benchmark formulas, each compiled on several vtrees: circuits from the CNF
//! corpus under `tests/data/`, and pigeonhole and random 3-CNF formulas made
//! by the generators in `tests/common/`.
//!
//! Each benchmark is a module with one test per vtree, so
//! `cargo test --test benchmarks cm42a` runs one benchmark on all of them.
//! A test checks the model count, the library's fast invariants, and that a
//! second minimize changes neither the function nor the size; a size given
//! with a vtree pins the canonical diagram size on it. On the balanced vtree
//! the test also compiles the clauses in reverse order and requires the same
//! size.

mod common;

use std::sync::Arc;

use num_bigint::BigUint;
use tididi::Engine;
use tididi::test_helpers::check::check_all_fast;
use tididi::test_helpers::check_minimize_soundness;
use tididi_bonsai::vtree::VtreeOptions;
use tididi_bonsai::compile::{CompileOptions, compile_cnf};
use tididi::vtree::Vtree;
use tididi_bonsai::vtree::build_vtree;
use vitri::cnf::CnfFormula;

/// A shape of [`common::VTREE_SHAPES`], or one of two constructions from a
/// spec: `minfill` and `flowcutter`.
fn vtree(shape: &str, formula: &CnfFormula) -> Arc<Vtree> {
    let spec = match shape {
        "minfill" => "minfill-primal",
        "flowcutter" => "flowcutter-primal:budget=1000steps,iters=10",
        _ => return common::vtree_shape(shape, formula.num_vars()),
    };
    build_vtree(formula, spec, &VtreeOptions::default()).unwrap()
}

/// The CNF at `path` under `tests/data/`.
fn cnf(path: &str) -> CnfFormula {
    common::load_cnf(&format!("tests/data/{path}"))
}

/// The pigeonhole formula for `pigeons` pigeons and `holes` holes.
fn pigeonhole(pigeons: i32, holes: i32) -> CnfFormula {
    let (num_vars, clauses) = common::pigeonhole_cnf(pigeons, holes);
    common::make_formula(num_vars, clauses)
}

fn check(name: &str, formula: &CnfFormula, shape: &str, models: u64, size: Option<usize>) {
    let eng = &Engine::new();
    let label = format!("{name} ({shape})");
    let vtree = vtree(shape, formula);
    let mut tdd = compile_cnf(eng, formula, &vtree, CompileOptions::default()).unwrap().diagram;

    let models = BigUint::from(models);
    assert_eq!(tdd.model_count().unwrap(), models, "{label}: model count");
    let pairs = tdd.pair_count();
    if let Some(size) = size {
        assert_eq!(pairs, size, "{label}: diagram size");
    }
    if shape == "balanced" {
        let mut reversed = formula.clauses().to_vec();
        reversed.reverse();
        let reversed = compile_cnf(eng, &common::formula(formula.num_vars(), reversed), &vtree, CompileOptions::default()).unwrap().diagram;
        assert_eq!(reversed.pair_count(), pairs, "{label}: size with the clauses reversed");
    }
    check_all_fast(&tdd, &label);
    check_minimize_soundness(&mut tdd, 3).unwrap_or_else(|e| panic!("{label}: minimize soundness: {e}"));
    assert_eq!(tdd.pair_count(), pairs, "{label}: a second minimize changed the size");
    assert_eq!(tdd.model_count().unwrap(), models, "{label}: a second minimize changed the model count");
}

/// `benchmark!(name, formula, models: shape [= size], ...)`.
macro_rules! benchmark {
    ($name:ident, $formula:expr, $models:literal: $($shape:ident $(= $size:literal)?),+) => {
        mod $name {
            use super::*;
            $(#[test] fn $shape() { check(stringify!($name), &$formula, stringify!($shape), $models, None $(.or(Some($size)))?); })+
        }
    };
}

benchmark!(tiny, cnf("tiny.cnf"), 48: balanced = 27, reverse_linear = 27, random42 = 38, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(c17, cnf("cnfs/LGSynth89/C17.cnf"), 32: balanced = 99, reverse_linear = 120, random42 = 118, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(majority, cnf("cnfs/LGSynth89/majority.cnf"), 32: balanced = 97, reverse_linear = 126, random42 = 88, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(b1, cnf("cnfs/LGSynth89/b1.cnf"), 8: balanced = 104, reverse_linear = 140, random42 = 105, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(cm82a, cnf("cnfs/LGSynth89/cm82a.cnf"), 32: balanced = 251, reverse_linear = 519, random42 = 269, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(cm152a, cnf("cnfs/LGSynth89/cm152a.cnf"), 2048: balanced = 378, reverse_linear = 1161, random42 = 191, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(pigeonhole_5_4, pigeonhole(5, 4), 0: balanced = 0, reverse_linear = 0, random42 = 0, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(cm42a, cnf("cnfs/LGSynth89/cm42a.cnf"), 16: balanced = 332, reverse_linear = 571, random42 = 327, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(cm138a, cnf("cnfs/LGSynth89/cm138a.cnf"), 64: balanced = 759, reverse_linear = 2626, random42 = 575, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(cm151a, cnf("cnfs/LGSynth89/cm151a.cnf"), 4096: balanced = 3077, reverse_linear = 10329, random42 = 2982, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(decod, cnf("cnfs/LGSynth89/decod.cnf"), 32: balanced = 250, reverse_linear = 538, random42 = 253, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(parity, cnf("cnfs/LGSynth89/parity.cnf"), 65536: balanced = 18564, reverse_linear = 100551, linear, random0, random1, random42, interleaved, minfill, flowcutter);
benchmark!(tcon, cnf("cnfs/LGSynth89/tcon.cnf"), 131072: reverse_linear = 1725101, random42 = 94827, random0, random1, interleaved, minfill, flowcutter);
benchmark!(pigeonhole_4_6, pigeonhole(4, 6), 3360: balanced = 1240, reverse_linear = 1513, random42 = 2475, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(rand_3_25_75_3, common::random_3cnf(25, 75, 3), 2091: balanced = 1832, reverse_linear = 2102, random42 = 1211, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(rand_3_25_100_1, common::random_3cnf(25, 100, 1), 28: balanced = 66, reverse_linear = 81, random42 = 67, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(rand_3_25_100_3, common::random_3cnf(25, 100, 3), 49: balanced = 155, reverse_linear = 222, random42 = 161, linear, random0, random1, interleaved, minfill, flowcutter);
benchmark!(x2, cnf("cnfs/LGSynth89/x2.cnf"), 1024: balanced = 1990, linear, random42);
benchmark!(cm163a, cnf("cnfs/LGSynth89/cm163a.cnf"), 65536: random42 = 20377, linear, reverse_linear, minfill, flowcutter);
benchmark!(cm162a, cnf("cnfs/LGSynth89/cm162a.cnf"), 16384: balanced = 35670, reverse_linear, random42, minfill, flowcutter);
benchmark!(cm85a, cnf("cnfs/LGSynth89/cm85a.cnf"), 2048: balanced = 6855, reverse_linear, random0, random1, minfill, flowcutter);
benchmark!(cmb, cnf("cnfs/LGSynth89/cmb.cnf"), 65536: balanced = 17344, linear, random42, minfill, flowcutter);
benchmark!(pcle, cnf("cnfs/LGSynth89/pcle.cnf"), 524288: balanced = 21108, linear, random42);
benchmark!(cm150a, cnf("cnfs/LGSynth89/cm150a.cnf"), 2097152: linear = 1680637, random1, minfill, flowcutter);
benchmark!(cu, cnf("cnfs/LGSynth89/cu.cnf"), 16384: balanced = 8466, random0, random42, minfill, flowcutter);
benchmark!(mux, cnf("cnfs/LGSynth89/mux.cnf"), 2097152: linear = 203363, interleaved, minfill, flowcutter);
benchmark!(pm1, cnf("cnfs/LGSynth89/pm1.cnf"), 65536: linear = 16469, random42, minfill, flowcutter);
benchmark!(cc, cnf("cnfs/LGSynth89/cc.cnf"), 2097152: minfill, flowcutter);
benchmark!(ldd, cnf("cnfs/LGSynth89/ldd.cnf"), 512: reverse_linear = 2511, interleaved, minfill, flowcutter);
benchmark!(pigeonhole_8_7, pigeonhole(8, 7), 0: balanced = 0, linear, random42, minfill, flowcutter);
benchmark!(rand_3_40_24_2, common::random_3cnf(40, 24, 2), 49341714432: balanced = 47179);
benchmark!(rand_3_40_28_2, common::random_3cnf(40, 28, 2), 29676369920: balanced = 43455);
benchmark!(pigeonhole_7_6, pigeonhole(7, 6), 0: balanced = 0, linear);
benchmark!(f51m, cnf("cnfs/LGSynth89/f51m.cnf"), 256: random42 = 3388, random0, minfill, flowcutter);
benchmark!(my_adder, cnf("cnfs/LGSynth89/my_adder.cnf"), 17179869184: minfill, flowcutter);
benchmark!(comp, cnf("cnfs/LGSynth89/comp.cnf"), 4294967296: minfill, flowcutter);
benchmark!(cht, cnf("cnfs/LGSynth89/cht.cnf"), 562949953421312: minfill, flowcutter);
benchmark!(pcler8, cnf("cnfs/LGSynth89/pcler8.cnf"), 134217728: random0, minfill, flowcutter);
benchmark!(count, cnf("cnfs/LGSynth89/count.cnf"), 68719476736: minfill, flowcutter);
benchmark!(rand_3_25_75_1, common::random_3cnf(25, 75, 1), 822: balanced = 679, linear, minfill, flowcutter);
benchmark!(rand_3_25_75_2, common::random_3cnf(25, 75, 2), 1005: balanced = 530, linear, random42, minfill, flowcutter);
benchmark!(rand_3_25_75_4, common::random_3cnf(25, 75, 4), 670: balanced = 589, random42);
benchmark!(rand_3_25_100_2, common::random_3cnf(25, 100, 2), 62: balanced = 124);
