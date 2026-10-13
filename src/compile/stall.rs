//! The per-step wall: a compile handed a [`StallWatch`]
//! ([`super::CompileRun::stall_watch`]) bounds each of its internal vtree-node
//! steps that the watch's [`StallRule`] covers by a wall of its own, counted
//! from the step's start. The step, the satisfiability check after it and the
//! marginalization and clustering that follow it all run with the compile's
//! deadline and the engine's cut to the step's. A step stopped there is a
//! *stall*, which the watch keeps for its owner to read once the compile has
//! given up. A step the wall cannot shorten, because the compile's own
//! deadline comes first, runs as it would without one, and so does every step
//! of a compile handed no watch.

use std::cell::Cell;
use std::time::{Duration, Instant};

use tididi::Engine;
use tididi::limits::LimitScope;
use tididi::vtree::VtreeIdx;

use crate::budget::deadline_scope;

/// Which internal steps of a compile get a wall of their own, and how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StallRule {
    /// How long one bounded step may run.
    pub(crate) wall: Duration,
    /// The fewest internal steps, this one included, that must be left in the
    /// component's bottom-up schedule for the wall to bound a step: the step at
    /// 0-based position `k` of `total` is bounded when `total - k >= min_left`.
    /// 0 bounds every step.
    pub(crate) min_left: usize,
}

impl StallRule {
    /// Whether the internal step at 0-based position `k` of a schedule of
    /// `total` steps gets the wall.
    pub(crate) fn bounds(&self, k: usize, total: usize) -> bool {
        total.saturating_sub(k) >= self.min_left
    }
}

/// One internal step that stopped on its own wall: where it was and how long it
/// had run when it stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stall {
    /// The vtree node the step was compiling.
    pub(crate) node: usize,
    /// Its 0-based position in the component's internal bottom-up schedule.
    pub(crate) k: usize,
    /// The length of that schedule.
    pub(crate) total: usize,
    /// How long the step had run when it stopped.
    pub(crate) ran: Duration,
}

/// A [`StallRule`] for one compile, and the stall it reports. The caller owns
/// it and hands the compile a reference; the compile records the step that
/// stops on its wall, and gives up there.
#[derive(Debug)]
pub(crate) struct StallWatch {
    rule: StallRule,
    stalled: Cell<Option<Stall>>,
}

impl StallWatch {
    pub(crate) fn new(rule: StallRule) -> Self {
        StallWatch { rule, stalled: Cell::new(None) }
    }

    /// The rule the compile runs under.
    pub(crate) fn rule(&self) -> StallRule {
        self.rule
    }

    /// The step that stopped on its wall, once one has.
    pub(crate) fn stall(&self) -> Option<Stall> {
        self.stalled.get()
    }

    /// The wall for internal step `t`, at position `k` of `total`, starting
    /// now: `None` when the rule does not bound the step, or when
    /// `job_deadline` comes no later than the wall would and the step needs
    /// none of its own. The engine's deadline is cut to the step's for as long
    /// as the returned step lives.
    pub(super) fn start_step<'a>(
        &'a self,
        eng: &'a Engine,
        t: VtreeIdx,
        k: usize,
        total: usize,
        job_deadline: Option<Instant>,
    ) -> Option<WalledStep<'a>> {
        if !self.rule.bounds(k, total) {
            return None;
        }
        let start = Instant::now();
        let deadline = start + self.rule.wall;
        if job_deadline.is_some_and(|d| d <= deadline) {
            return None;
        }
        let _limits = deadline_scope(eng, Some(deadline));
        Some(WalledStep { watch: self, node: t.idx(), k, total, start, deadline, _limits })
    }
}

/// One internal step running under its wall ([`StallWatch::start_step`]).
pub(super) struct WalledStep<'a> {
    watch: &'a StallWatch,
    node: usize,
    k: usize,
    total: usize,
    start: Instant,
    /// When the step's wall ends: the deadline its step context and the
    /// engine run under.
    pub(super) deadline: Instant,
    /// The engine's deadline cut to the step's, restored on drop.
    _limits: Option<LimitScope<'a>>,
}

impl WalledStep<'_> {
    /// The step stopped. Once its wall has passed, that is a stall: the
    /// compile's deadline is later than the wall ([`StallWatch::start_step`]),
    /// so nothing earlier than the wall stopped it.
    pub(super) fn note_stop(&self) {
        let now = Instant::now();
        if now >= self.deadline {
            self.watch.stalled.set(Some(Stall {
                node: self.node,
                k: self.k,
                total: self.total,
                ran: now.duration_since(self.start),
            }));
        }
    }
}

#[cfg(test)]
#[path = "tests/stall.rs"]
mod tests;
