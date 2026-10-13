use super::*;
use crate::test_support::{formula, lit};
use tididi::test_helpers::Lcg;

/// The direct hash-set implementation of the tour, the reference order
/// `proximity_order` must reproduce.
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

        let sets = DenseClauseVars::new(&group, &formula);
        widths_seen[match sets.ids.len().div_ceil(64) {
            0 | 1 => 0,
            2 => 1,
            3 | 4 => 2,
            _ => 3,
        }] = true;

        let mut expected = group.clone();
        reference_reorder(&mut expected, &formula);
        let tour: Vec<usize> = proximity_order(&sets).iter().map(|&k| group[k]).collect();
        assert_eq!(tour, expected, "case {case}: {num_clauses} clauses over a pool of {pool_size}");
    }
    assert_eq!(widths_seen, [true; 4], "every overlap representation was exercised");
}

/// The completion greedy as first written: every placement rescores every
/// unplaced clause from the variables' state. Returns group positions.
fn reference_completion(group: &[usize], formula: &CnfFormula, finishes: impl Fn(VarId) -> bool) -> Vec<usize> {
    use rustc_hash::{FxHashMap, FxHashSet};
    let vars_of = |k: usize| formula.clauses()[group[k]].literals.iter().map(|l| l.var);
    let mut pending: FxHashMap<VarId, i64> = FxHashMap::default();
    for k in 0..group.len() {
        for v in vars_of(k) {
            *pending.entry(v).or_default() += 1;
        }
    }
    let mut placed = vec![false; group.len()];
    let mut pendants = Vec::new();
    for (k, placed) in placed.iter_mut().enumerate() {
        if vars_of(k).all(|v| pending[&v] == 1 && finishes(v)) {
            *placed = true;
            pendants.push(k);
        }
    }
    let mut open: FxHashSet<VarId> = FxHashSet::default();
    let mut order = Vec::new();
    while order.len() + pendants.len() < group.len() {
        let mut best: Option<(i64, i64, usize)> = None;
        for k in (0..group.len()).filter(|&k| !placed[k]) {
            let (mut grow, mut done) = (0, 0);
            for v in vars_of(k) {
                let fin = finishes(v) && pending[&v] == 1;
                if !open.contains(&v) && !fin {
                    grow += 1;
                }
                if open.contains(&v) && fin {
                    done += 1;
                }
            }
            let key = (grow - done, -done, k);
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        }
        let k = best.unwrap().2;
        placed[k] = true;
        for v in vars_of(k) {
            let p = pending.get_mut(&v).unwrap();
            *p -= 1;
            let fin = finishes(v) && *p == 0;
            if !open.contains(&v) && !fin {
                open.insert(v);
            } else if open.contains(&v) && fin {
                open.remove(&v);
            }
        }
        order.push(k);
    }
    order.extend(pendants);
    order
}

/// The open profile as first written, over hash sets; `order` holds group
/// positions.
fn reference_profile(
    order: &[usize],
    group: &[usize],
    formula: &CnfFormula,
    finishes: impl Fn(VarId) -> bool,
) -> OpenProfile {
    use rustc_hash::{FxHashMap, FxHashSet};
    let vars_of = |k: usize| formula.clauses()[group[k]].literals.iter().map(|l| l.var);
    let mut pending: FxHashMap<VarId, i64> = FxHashMap::default();
    for &k in order {
        for v in vars_of(k) {
            *pending.entry(v).or_default() += 1;
        }
    }
    let mut open: FxHashSet<VarId> = FxHashSet::default();
    let mut profile = OpenProfile::default();
    for &k in order {
        for v in vars_of(k) {
            *pending.get_mut(&v).unwrap() -= 1;
            open.insert(v);
            if pending[&v] == 0 && finishes(v) {
                open.remove(&v);
            }
        }
        profile.max = profile.max.max(open.len());
        profile.sum += open.len();
    }
    profile
}

/// The incremental completion greedy places clauses as the direct one does,
/// both orders score as the direct profile does, each is a permutation of
/// the group, and `order_group` picks one of them.
#[test]
fn completion_order_matches_the_direct_greedy_and_orders_are_permutations() {
    let mut rng = Lcg::new(0xc0de_0d3e);
    // One scratch for every case, as one compile reuses it across nodes.
    let mut scratch = OrderScratch::default();
    for case in 0..300 {
        let num_clauses = draw(&mut rng, 1..150);
        let pool_size = [3, 8, 20, 64, 200][case % 5];
        let max_len = [1, 2, 3, 5, 12][draw(&mut rng, 0..5)];
        let block = if chance(&mut rng, 25) { draw(&mut rng, 1..8) } else { 0 };
        let dup_percent = [0, 10, 50][draw(&mut rng, 0..3)];
        let formula = random_formula(&mut rng, num_clauses + 10, pool_size, max_len, block, dup_percent);
        let mut group: Vec<usize> = (0..formula.clauses().len()).collect();
        shuffle(&mut rng, &mut group);
        group.truncate(num_clauses);
        let finish_percent = [0, 50, 90, 100][case % 4];
        let finishing: rustc_hash::FxHashSet<usize> =
            (0..pool_size * 7).filter(|_| chance(&mut rng, finish_percent)).collect();
        let finishes = |var: VarId| finishing.contains(&var.idx());

        let sets = DenseClauseVars::new(&group, &formula);
        scratch.load(&sets, finishes);
        let completion = scratch.completion_order(&sets);
        assert_eq!(completion, reference_completion(&group, &formula, finishes), "case {case}");
        let tour = proximity_order(&sets);
        let positions: Vec<usize> = (0..group.len()).collect();
        for order in [&tour, &completion] {
            assert_eq!(
                scratch.profile(&sets, order),
                reference_profile(order, &group, &formula, finishes),
                "case {case}"
            );
            let mut sorted = order.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, positions, "case {case}: not a permutation of the group");
        }

        let as_clauses = |order: &[usize]| order.iter().map(|&k| group[k]).collect::<Vec<_>>();
        let mut ordered = group.clone();
        order_group(&mut ordered, &formula, finishes, &mut scratch);
        assert!(ordered == as_clauses(&tour) || ordered == as_clauses(&completion), "case {case}");
    }
}

/// A formula over variables `0..num_vars` (zero-based) whose clauses are
/// `clauses`, every literal positive.
fn positive_formula(num_vars: u32, clauses: &[Vec<u32>]) -> CnfFormula {
    formula(num_vars, clauses.iter().map(|c| Clause::new(c.iter().map(|&v| lit(v, true)).collect())).collect())
}

/// The two candidate orders of the whole formula as one group, in clause
/// indices, with their open profiles. Variable v (zero-based) finishes at
/// the node where `finishes(v)`.
struct Candidates {
    tour: Vec<usize>,
    completion: Vec<usize>,
    tour_profile: OpenProfile,
    completion_profile: OpenProfile,
    any_finishes: bool,
}

fn candidates(formula: &CnfFormula, finishes: impl Fn(u32) -> bool) -> Candidates {
    let group: Vec<usize> = (0..formula.clauses().len()).collect();
    let sets = DenseClauseVars::new(&group, formula);
    let mut scratch = OrderScratch::default();
    scratch.load(&sets, |var| finishes(var.idx() as u32));
    let tour = proximity_order(&sets);
    let completion = scratch.completion_order(&sets);
    Candidates {
        tour_profile: scratch.profile(&sets, &tour),
        completion_profile: scratch.profile(&sets, &completion),
        any_finishes: scratch.finishes.contains(&true),
        tour,
        completion,
    }
}

/// The order `order_group` gives the whole formula as one group.
fn chosen(formula: &CnfFormula, finishes: impl Fn(u32) -> bool) -> Vec<usize> {
    let mut group: Vec<usize> = (0..formula.clauses().len()).collect();
    order_group(&mut group, formula, |var| finishes(var.idx() as u32), &mut OrderScratch::default());
    group
}

/// A chain of `n` clauses (x_i ∨ x_{i+1}) over x_0..x_n, in chain order
/// except that clause `start` moves to the end, where the tour starts (the
/// last of the largest clauses). The tour walks from it to the far end, then
/// jumps back and walks to x_0, keeping x_start open across the jump; the
/// completion order walks the chain from x_0, one variable open.
fn chain(n: usize, start: usize) -> CnfFormula {
    let mut clauses: Vec<Vec<u32>> = (0..n as u32).map(|i| vec![i, i + 1]).collect();
    let moved = clauses.remove(start);
    clauses.push(moved);
    positive_formula(n as u32 + 1, &clauses)
}

#[test]
fn prefer_completion_thresholds() {
    let p = |max, sum| OpenProfile { max, sum };
    let tour = p(10, 100);
    assert!(prefer_completion(p(9, 80), tour, true), "both margins met exactly");
    assert!(!prefer_completion(p(9, 81), tour, true), "the sum misses its margin");
    assert!(!prefer_completion(p(10, 50), tour, true), "the max misses its margin");
    assert!(prefer_completion(p(10, 50), tour, false), "no variable finishes: a smaller sum is enough");
    assert!(!prefer_completion(p(10, 100), tour, false), "an equal sum keeps the tour");
    assert!(!prefer_completion(p(0, 0), p(0, 0), true), "nothing open in either order keeps the tour");
}

#[test]
fn completion_order_places_pendants_last() {
    // a b c = 0 1 2 form a triangle; [p q] = [3 4] and [r s] = [5 6] share
    // nothing with it or each other.
    let formula = positive_formula(7, &[vec![3, 4], vec![0, 1], vec![1, 2], vec![0, 2], vec![5, 6]]);
    // Every variable finishes here: both [p q] and [r s] are pendants. The
    // greedy would place them first, at no open cost; they go last, in group
    // order.
    assert_eq!(candidates(&formula, |_| true).completion, vec![1, 2, 3, 0, 4]);
    // s finishes higher up, so [r s] is no pendant: it leaves only s open and
    // the greedy places it first.
    assert_eq!(candidates(&formula, |v| v != 6).completion, vec![4, 1, 2, 3, 0]);
}

#[test]
fn the_rule_takes_a_clearly_smaller_completion_order() {
    let formula = chain(41, 20);
    let c = candidates(&formula, |_| true);
    assert_eq!(c.tour_profile, OpenProfile { max: 2, sum: 60 });
    assert_eq!(c.completion_profile, OpenProfile { max: 1, sum: 40 });
    assert_ne!(c.tour, c.completion);
    assert_eq!(chosen(&formula, |_| true), c.completion);
}

#[test]
fn the_rule_keeps_the_tour_on_a_near_tie() {
    // Starting near the far end, the tour keeps two variables open only
    // briefly: the completion order is smaller on both counts, but its sum
    // misses the margin.
    let formula = chain(41, 37);
    let c = candidates(&formula, |_| true);
    assert_eq!(c.tour_profile, OpenProfile { max: 2, sum: 43 });
    assert_eq!(c.completion_profile, OpenProfile { max: 1, sum: 40 });
    assert_ne!(c.tour, c.completion);
    assert_eq!(chosen(&formula, |_| true), c.tour);
}

#[test]
fn the_rule_without_a_finishing_variable_needs_only_a_smaller_sum() {
    // A wide clause and three of its variables as units, no variable
    // finishing here: the tour opens all four at once, the completion order
    // one at a time. The maxima tie, so the margin rule alone would keep the
    // tour.
    let formula = positive_formula(4, &[vec![0, 1, 2, 3], vec![0], vec![1], vec![2]]);
    let c = candidates(&formula, |_| false);
    assert!(!c.any_finishes);
    assert_eq!((c.tour.clone(), c.tour_profile), (vec![0, 3, 2, 1], OpenProfile { max: 4, sum: 16 }));
    assert_eq!((c.completion.clone(), c.completion_profile), (vec![1, 2, 3, 0], OpenProfile { max: 4, sum: 10 }));
    assert!(!prefer_completion(c.completion_profile, c.tour_profile, true));
    assert_eq!(chosen(&formula, |_| false), c.completion);
}

#[test]
fn groups_over_the_cap_keep_the_tour() {
    let n = COMPLETION_ORDER_MAX_CLAUSES + 1;
    let formula = chain(n, n / 2);
    let c = candidates(&formula, |_| true);
    assert!(
        prefer_completion(c.completion_profile, c.tour_profile, c.any_finishes),
        "under the cap this group would take the completion order"
    );
    assert_ne!(c.tour, c.completion);
    assert_eq!(chosen(&formula, |_| true), c.tour);
}
