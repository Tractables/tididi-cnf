use std::sync::Arc;
use std::time::{Duration, Instant};

use num_bigint::BigUint;
use tididi::Engine;
use tididi::limits::StopAt;
use tididi::vtree::Vtree;

use super::*;
use crate::compile::component::{CompileJob, PassOutput};
use crate::compile::step::compile_component;
use crate::compile::{CompileRun, GiveUp};
use crate::test_support::{self as fixtures, clause};

/// A connected 3-CNF over eight variables: its clauses land at internal
/// vtree nodes, so the internal steps have work to do.
const CONNECTED: &[&[i32]] = &[
    &[1, 2, 3],
    &[-2, 4, 5],
    &[3, -5, 6],
    &[-1, 6, 7],
    &[4, -7, 8],
    &[-3, 5, -8],
    &[2, -6, 8],
    &[-4, 7, 1],
];

fn connected() -> vitri::cnf::CnfFormula {
    fixtures::formula(8, CONNECTED.iter().map(|c| clause(c)).collect())
}

/// The internal schedule of the balanced vtree the pass runs on.
fn schedule() -> Vec<tididi::vtree::VtreeIdx> {
    Vtree::balanced(8).internal_bottomup_slice().to_vec()
}

/// The counting pass over [`CONNECTED`] on a balanced vtree, under `rule`,
/// with an hour's job deadline: its count, or the give-up, and what the
/// watch reports.
fn run_watched(eng: &Engine, rule: StallRule) -> (Result<BigUint, GiveUp>, Option<Stall>) {
    let formula = connected();
    let vtree = Arc::new(Vtree::balanced(8));
    let watch = StallWatch::new(rule);
    let prune_spent_us = std::cell::Cell::new(0);
    let job = CompileJob {
        deadline: Some(Instant::now() + Duration::from_secs(3600)),
        output: PassOutput::Count(None),
        run: CompileRun { stall_watch: Some(&watch), ..CompileRun::default() },
        prune_spent_us: &prune_spent_us,
    };
    let count = compile_component(eng, &formula, &vtree, job)
        .map(|compiled| compiled.projected_count(eng).expect("the count reads"));
    (count, watch.stall())
}

/// A step under a wall that has already passed stops, and the watch names
/// it: the only step the rule bounds here is the first.
#[test]
fn a_step_past_its_wall_stops_the_pass_and_is_reported() {
    let schedule = schedule();
    let rule = StallRule { wall: Duration::ZERO, min_left: schedule.len() };
    let (count, stall) = run_watched(&Engine::new(), rule);
    assert_eq!(count, Err(GiveUp::Stopped), "a stalled step gives the pass up");
    let stall = stall.expect("the stall is reported");
    assert_eq!((stall.node, stall.k, stall.total), (schedule[0].idx(), 0, schedule.len()));
}

/// The rule bounds a step only with enough steps left: with more asked for
/// than the schedule holds, no step has a wall, and the pass answers the
/// formula's count however short the wall.
#[test]
fn a_step_the_rule_does_not_bound_runs_without_a_wall() {
    let rule = StallRule { wall: Duration::ZERO, min_left: schedule().len() + 1 };
    let (count, stall) = run_watched(&Engine::new(), rule);
    assert_eq!(stall, None);
    assert_eq!(count, Ok(fixtures::brute_projected_count(&connected(), 0..8)));
}

/// A wall no step reaches changes nothing: the pass answers the count.
#[test]
fn a_wall_no_step_reaches_leaves_the_count_alone() {
    let rule = StallRule { wall: Duration::from_secs(600), min_left: 0 };
    let (count, stall) = run_watched(&Engine::new(), rule);
    assert_eq!(stall, None);
    assert_eq!(count, Ok(fixtures::brute_projected_count(&connected(), 0..8)));
}

/// A step gets a wall of its own only when the rule bounds its position and
/// the wall ends before the job's deadline, so a stop at the job's deadline
/// is never read as a stall; while it has one, the engine stops at the
/// step's deadline.
#[test]
fn a_step_gets_its_own_wall_only_ahead_of_the_jobs_deadline() {
    let eng = &Engine::new();
    let t = schedule()[0];
    let watch = StallWatch::new(StallRule { wall: Duration::from_secs(600), min_left: 0 });
    let now = Instant::now();
    assert!(watch.start_step(eng, t, 0, 10, Some(now + Duration::from_secs(1))).is_none());
    assert!(watch.start_step(eng, t, 0, 10, Some(now + Duration::from_secs(3600))).is_some());
    assert!(watch.start_step(eng, t, 0, 10, None).is_some());
    let gated = StallWatch::new(StallRule { wall: Duration::from_secs(600), min_left: 11 });
    assert!(gated.start_step(eng, t, 0, 10, None).is_none(), "ten steps left, eleven asked for");
    {
        let step = watch.start_step(eng, t, 0, 10, None).unwrap();
        assert_eq!(eng.limits().armed().stop_rules().unconditional, Some(StopAt::Time(step.deadline)));
    }
    assert_eq!(eng.limits().armed().stop_rules().unconditional, None, "the step's deadline is gone with it");
}

/// A stop before the step's wall is not a stall: something else stopped it.
#[test]
fn a_stop_before_the_wall_is_not_a_stall() {
    let eng = &Engine::new();
    let watch = StallWatch::new(StallRule { wall: Duration::from_secs(600), min_left: 0 });
    watch.start_step(eng, schedule()[0], 0, 10, None).unwrap().note_stop();
    assert_eq!(watch.stall(), None);
}
