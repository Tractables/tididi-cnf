# The support-enumeration stage

For plain (unweighted, non-projected) model counting. Module:
`src/stage/mc/support_enum.rs`, whose module doc gives the phases and whose
item docs give each constant's value and reason. Hooks: the primary
compile's fallbacks in `src/stage/mc/cascade.rs`, and the count without a
reduction in `src/stage/mc/mod.rs`.

The stage runs only after the compile has failed, as a fallback. Behind
the primary compile the hook tries the no-SBVA retry, then support
enumeration, then the unreduced retry; behind the count without a reduction,
support enumeration alone. When they fail, the Shannon-split recovery cascade
runs.

## The problem it solves

Some instances are cheap to *count* and expensive to *compile*. The recovery
cascade answers a blown-up compile by Shannon-splitting the formula and
compiling the halves. That works when the blowup is a diagram-size problem, and
fails when the formula's model set is small but its diagram is not: the cascade
spends the time it has left retrying the method that already failed.

By then Arjun has handed the driver a minimized **independent support** of the
reduced formula. If that support has few feasible assignments, they can be
enumerated.

## The mechanism

The stage enumerates the feasible assignments to Arjun's independent support of
the reduced formula through the projected-ALL-SAT engine (`allsat`), driven
through `show_enum_fastpath`.

Whether to enter is the hard part, because the separating quantity — the
number of feasible support projections N — is not predictable from anything
static: support size, support fraction, reduced formula size and enumeration
rate all overlap between the instances this wins and the ones it loses. So the
stage estimates N (`estimate_projected_count`), probes for `PROBE_WALL_MS`, and
commits the rest of its window only when the estimate is at most
`COMMIT_MAX_MODELS` (`commit_decision`, asked once before the probe and once
when the probe's time is up, on the same estimate). A commit resumes the
probe's solver.

The estimate walks random paths down the projection tree. Fixing a random
subset of support variables to random polarities and scaling by `2^j` is also
unbiased, but these supports are mostly near-forced variables, so a random
polarity vector almost always contradicts the formula and the estimate
collapses to zero, which reads as "small". A path estimate cannot collapse: a
forced variable contributes a factor of one.

## Why it is sound

Arjun's sampling set is an independent support of the reduced formula: every
remaining variable is determined by it, so each feasible support assignment
extends to exactly one model, and enumerating them counts the reduced formula
exactly. Variables absent from every clause are lifted by the enumerator's
free-show factor. The reduced count is lifted by the reduction's Arjun
multiplier where the stage returns, and the driver applies its count lift on
top, as for every other MC answer.

The estimate only decides whether to keep enumerating; a count is emitted only
when the enumeration runs out of feasible projections. A mis-estimate costs
time, never correctness.

## What a loss costs

- The stage never touches the cascade's reserve: `plan_support_enum` carves
  its window out of what `shannon_split_reserve_ms` leaves, and declines when
  that window is too short.
- A candidate the estimate rejects costs the estimator only.
- A candidate the estimate admits and the probe cannot finish costs both,
  each within its own time budget, and the cascade continues on the reserve
  plus the unspent window.
- A wrong commit is bounded by the window and by `TALLY_CEILING`, which does
  not depend on the estimate. The deadline holds inside a solve too, through
  the terminator the engine connects when a caller supervises the
  enumeration (`AllsatCtl`).

The entry decision cannot be left to the deadline: the Shannon cascade behind
the stage stops only at the program deadline, so every second an early exit
frees goes to it, and a wrong commit that runs to the end of the window
costs the whole window.

## The count without a reduction

That count is reached whenever Arjun kept no reduction — it was killed at its
deadline, gave up, was skipped, or was discarded as not smaller or
non-injective — so it has no support to hand the stage. The hook buys one with
a second, tightly bounded Arjun pass on the recovery window
(`reduce_for_support`), sized against the same `shannon_split_reserve_ms` rule.
