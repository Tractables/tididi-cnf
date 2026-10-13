//! The conditioning recursion is checked by forcing it to condition on the
//! hub show var and asserting the branch-sum equals both an independent
//! brute-force projected count and the direct depth-0 leaf (the identity
//! PMC(F) = Σ_cutset PMC(F|assignment)). `arjun_ms = 0` keeps the leaf
//! hermetic (no external Arjun).
use super::*;

use tididi::Engine;
use vitri::cnf::Clause;
use crate::test_support::{brute_projected_count, lit};

/// show = {0,1,2}, hidden = {3}. x0 is the hub (occurs most): under x0=T the
/// clause ¬x0∨x2 forces the show var x2=T, and the x0=F branch is UNSAT —
/// (x0∨x3)∧(x0∨¬x3) forces x3 both ways ⊢ □. So a shallow single-var cutset on x0
/// is a clean partition ({x0=T}, {x0=F UNSAT}): one branch carries the whole
/// count, the other's leaf compile must return zero.
fn fixture() -> CnfFormula {
    crate::test_support::formula(4, vec![
        Clause::new(vec![lit(0, true), lit(3, true)]), // x0 ∨ x3   ┐ x0=F ⊢ □
        Clause::new(vec![lit(0, true), lit(3, false)]), // x0 ∨ ¬x3  ┘
        Clause::new(vec![lit(0, false), lit(2, true)]), // ¬x0 ∨ x2  (x0=T BCP-forces x2)
        Clause::new(vec![lit(1, true), lit(2, true)]), // x1 ∨ x2   (coupling)
        Clause::new(vec![lit(1, false), lit(2, true), lit(3, true)]), // ¬x1 ∨ x2 ∨ x3
    ])
}

#[test]
fn integer_pmc_conditioning_matches_brute_and_direct_leaf() {
    let eng = &Engine::new();
    let config = crate::test_support::minimal_config();
    let run = &crate::test_support::run(eng, &config);
    let f = fixture();
    let show = ShowSet::from_dimacs_ids(&[1, 2, 3]).unwrap();
    let brute_count = brute_projected_count(&f, [0, 1, 2]);

    // Direct depth-0 leaf (no conditioning): the true projected count.
    let direct = pmc_branch_count(run, &f, &show, None, 0)
        .expect("direct leaf");
    assert_eq!(
        direct, brute_count,
        "direct leaf must equal brute projected count"
    );

    // Forced conditioning over the whole cutset must reproduce it.
    let cutset = crate::enumeration::constrained_show_count(&f, &show);
    let cfg = ConditioningLimits {
        min_depth: cutset,
        depth_cap: cutset,
        // An hour: a wall the leaves never reach.
        branch_ms: 3_600_000,
        arjun_ms: 0,
        deadline: None,
    };
    let got = condition_and_count(run, &f, &show, 0, &cfg)
        .expect("conditioning must produce a count");
    assert_eq!(
        got, brute_count,
        "PMC conditioning branch-sum must equal brute count"
    );
}

/// A formula wide enough that its projected compile is many steps rather than
/// one, which is what makes a wall observable at all: a single-step compile
/// has nowhere to check one.
fn wide_formula(num_vars: u32, num_clauses: usize) -> CnfFormula {
    let mut rng = tididi::test_helpers::Lcg::new(0x2545_F491_4F6C_DD1D);
    let mut clauses = Vec::with_capacity(num_clauses);
    while clauses.len() < num_clauses {
        let mut vars: Vec<u32> = Vec::new();
        while vars.len() < 3 {
            let v = rng.below(u64::from(num_vars)) as u32;
            if !vars.contains(&v) {
                vars.push(v);
            }
        }
        let mut lits: Vec<vitri::cnf::Literal> = vars
            .iter()
            .map(|&v| vitri::cnf::Literal::new(vitri::cnf::VarId::new(v + 1).unwrap(), rng.coin()))
            .collect();
        lits.sort_by_key(|l| l.var);
        clauses.push(vitri::cnf::Clause::new(lits));
    }
    crate::test_support::formula(num_vars, clauses)
}

/// **The projected compile answers to the deadline it is handed.** The
/// last-ditch compile runs it outside every `catch_unwind` on the path, so with
/// no wall a compile started just before the deadline would run to the
/// external timeout: a deadline already gone declines, and no deadline
/// compiles without one.
#[test]
fn the_projected_compile_answers_to_the_deadline_it_is_handed() {
    let eng = &Engine::new();
    let config = crate::test_support::minimal_config();
    let run = &crate::test_support::run(eng, &config);
    let f = wide_formula(40, 150);
    let vtree =
        crate::vtree::build_vtree(&f, "minfill-primal", &crate::vtree::VtreeOptions::default()).unwrap();
    let forget: Vec<tididi::vtree::VarId> = (1..=8).map(tididi::vtree::VarId).collect();

    let g = projected_compile(
        run,
        &f,
        &vtree,
        crate::compile::ComponentPlan::Whole,
        &forget,
        None,
    )
    .expect("an unbounded projected compile declined");
    assert!(
        g > num_bigint::BigUint::ZERO,
        "the fixture compiled to zero; the test guards nothing"
    );

    let past = Instant::now() - std::time::Duration::from_secs(1);
    assert!(
        projected_compile(
            run,
            &f,
            &vtree,
            crate::compile::ComponentPlan::Whole,
            &forget,
            Some(past),
        )
        .is_none(),
        "the projected compile ran past a deadline that had already gone",
    );
}

/// A run with every variable shown carries no show set, so it cannot reach
/// conditioning; an empty one leaves nothing to split on.
#[test]
fn conditioning_declines_an_empty_show_set() {
    use vitri::cnf::{Clause, Literal};
    let lit = |v: u32| Literal::new(vitri::cnf::VarId::new(v + 1).unwrap(), true);
    let f = crate::test_support::formula(3, vec![
        Clause::new(vec![lit(0), lit(1)]),
        Clause::new(vec![lit(1), lit(2)]),
    ]);
    let eng = &Engine::new();
    let config = crate::test_support::minimal_config();
    let empty = ShowSet::empty();
    assert_eq!(condition_on_show(&crate::test_support::run(eng, &config), &f, &empty), None);
}
