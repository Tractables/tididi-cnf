use super::*;
use crate::test_support::{formula, lit};
use tididi::test_helpers::Lcg;

/// The direct hash-set implementation of the tour, the reference order
/// `reorder_by_variable_proximity` must reproduce.
fn reference_reorder(clauses: &mut [usize], formula: &CnfFormula) {
    use rustc_hash::FxHashSet;
    let n = clauses.len();
    let clause_vars: Vec<FxHashSet<u32>> = clauses
        .iter()
        .map(|&ci| formula.clauses()[ci].literals.iter().map(|l| l.var.get()).collect())
        .collect();

    let mut placed = vec![false; n];
    let mut new_order: Vec<usize> = Vec::with_capacity(n);
    let start = (0..n).max_by_key(|&i| clause_vars[i].len()).unwrap();
    placed[start] = true;
    new_order.push(start);
    while new_order.len() < n {
        let last_vs = &clause_vars[*new_order.last().unwrap()];
        let best = (0..n)
            .filter(|&i| !placed[i])
            .max_by_key(|&i| clause_vars[i].intersection(last_vs).count())
            .unwrap();
        placed[best] = true;
        new_order.push(best);
    }

    let reordered: Vec<usize> = new_order.iter().map(|&src| clauses[src]).collect();
    clauses.copy_from_slice(&reordered);
}

/// A draw in `range`.
fn draw(rng: &mut Lcg, range: std::ops::Range<usize>) -> usize {
    range.start + rng.below((range.end - range.start) as u64) as usize
}

/// Whether a draw lands under `percent` out of a hundred.
fn chance(rng: &mut Lcg, percent: u64) -> bool {
    rng.below(100) < percent
}

/// `items` in a random order.
fn shuffle<T>(rng: &mut Lcg, items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, draw(rng, 0..i + 1));
    }
}

/// A random formula of `num_clauses` clauses over a pool of `pool_size`
/// scattered variable ids. `block` > 0 confines each clause to one block of
/// that many pool variables, so most pairs share nothing; `dup_percent` is the
/// chance, out of a hundred, that a clause repeats an earlier one's variables.
fn random_formula(
    rng: &mut Lcg,
    num_clauses: usize,
    pool_size: usize,
    max_len: usize,
    block: usize,
    dup_percent: u64,
) -> CnfFormula {
    let mut ids: Vec<u32> = (0..(pool_size as u32) * 7).collect();
    shuffle(rng, &mut ids);
    ids.truncate(pool_size);
    let mut clauses: Vec<Clause> = Vec::with_capacity(num_clauses);
    for _ in 0..num_clauses {
        if !clauses.is_empty() && chance(rng, dup_percent) {
            let earlier = &clauses[draw(rng, 0..clauses.len())];
            let literals = earlier.literals.iter().map(|l| l.var.idx() as u32).collect::<Vec<_>>();
            clauses.push(Clause::new(literals.into_iter().map(|v| lit(v, rng.coin())).collect()));
            continue;
        }
        let candidates: &[u32] = if block > 0 {
            let b = draw(rng, 0..pool_size.div_ceil(block));
            &ids[b * block..((b + 1) * block).min(pool_size)]
        } else {
            &ids
        };
        let len = draw(rng, 1..max_len.min(candidates.len()) + 1);
        let mut vars = candidates.to_vec();
        shuffle(rng, &mut vars);
        vars.truncate(len);
        clauses.push(Clause::new(vars.into_iter().map(|v| lit(v, rng.coin())).collect()));
    }
    formula((pool_size as u32) * 7, clauses)
}

/// The bitset and marker-array tours place clauses in exactly the order of
/// the direct version, over every overlap representation.
#[test]
fn proximity_reorder_matches_the_hash_set_reference() {
    let mut rng = Lcg::new(0x0b0e_5a17);
    let mut widths_seen = [false; 4];
    for case in 0..600 {
        let num_clauses = match case % 4 {
            0 => draw(&mut rng, 1..12),
            1 | 2 => draw(&mut rng, 3..120),
            _ => draw(&mut rng, 120..400),
        };
        let pool_size = [3, 8, 41, 63, 64, 65, 100, 128, 129, 200, 251, 256, 257, 400, 1500][case % 15];
        let max_len = [1, 2, 3, 5, 12, 64][draw(&mut rng, 0..6)];
        let block = if chance(&mut rng, 25) { draw(&mut rng, 1..8) } else { 0 };
        let dup_percent = [0, 10, 50][draw(&mut rng, 0..3)];
        let formula = random_formula(&mut rng, num_clauses + 20, pool_size, max_len, block, dup_percent);

        // A group is any subset of the formula's clauses, in any order.
        let mut group: Vec<usize> = (0..formula.clauses().len()).collect();
        shuffle(&mut rng, &mut group);
        group.truncate(num_clauses);
        if rng.coin() {
            group.sort_unstable();
        }

        let words = DenseClauseVars::new(&group, &formula).num_vars.div_ceil(64);
        widths_seen[match words {
            0 | 1 => 0,
            2 => 1,
            3 | 4 => 2,
            _ => 3,
        }] = true;

        let mut expected = group.clone();
        reference_reorder(&mut expected, &formula);
        reorder_by_variable_proximity(&mut group, &formula);
        assert_eq!(group, expected, "case {case}: {num_clauses} clauses over a pool of {pool_size}");
    }
    assert_eq!(widths_seen, [true; 4], "every overlap representation was exercised");
}
