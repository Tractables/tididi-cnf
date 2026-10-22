# Measurement details

## Inputs and compiler

The primary comparisons contain all 1,400 MCC 2020–2026 track-1 entries,
representing 1,153 formulas. Formula identity uses the signed clause multiset,
declared variable count and task directives. Comments and clause/literal order
do not affect identity. Representative selection prefers an available measured
input, then the first competition key, independently of solve outcomes.
Repeated entries take their representative's status and time.

The compiler uses snapshot
[`f8acdab`](https://github.com/Tractables/tididi-cnf/tree/f8acdab8ae4f5becee2c8996905b844f17331efd)
and its locked dependencies. Both compilation panels use identical TDD records:
`tididi --mode compile --save-tdd <file> --time-limit-ms <remaining-ms>
--mem-limit-mb 61440 <instance>`. Input decompression and normalization are
inside its 120 s budget. Normalization removes task directives and comments,
preserving variables, literals and clause order. Default simplification and
portfolio vtree selection remain enabled. Counting uses `--mode mc` with
`--time-limit-ms 120000 --mem-limit-mb 61440`.

## Decision-diagram packages

[bdd-benchmark](https://github.com/SSoelvsten/bdd-benchmark/tree/3c521e16ada406d37ae5f960096a567a41df33b9)
conjoins clauses in a balanced tree. OxiDD 0.11.2 (`9158645e`), CUDD 3.0.0 +
20 commits (`538810a`) and Sylvan 1.9.0 (`8ddb68e3`) use complement edges;
BuDDy (`5aca063a`) and lib-bdd-ffi (`0c9481b4`) use plain BDDs. All use static
orders; CUDD is also measured with dynamic variable-order sifting enabled
after clause construction. Both CUDD configurations use the same prepared
inputs, source revision, original hosts and resource limits. Sifting time is
included in compilation. The harness activates `CUDD_REORDER_SIFT` for `-R`;
activation and exact counts are checked on small inputs before the suite.
The full suite measures compilation without an additional model-count query.

PySDD 1.0.6 supplies the official SDD CLI: `-r 0` disables vtree
search; the dynamic variant uses its default search settings.

Public vitri prepares FORCE variable/clause orders and initial SDD vtrees
outside timing, without preprocessing the packages' clauses. Failed preparation
uses original BDD variable numbering or the official default SDD initial tree.

## Resources and timing

Every invocation uses one CPU and a 60 GiB address-space cap. All hosts use
Xeon Platinum 8488C CPUs. BDD/SDD packages and tididi-cnf use `r7i.48xlarge`
(1.5 TiB RAM), 21 concurrent instances, TSC clocks and a 1 GiB stack.
The paired measurement of MCC 2026 instance 190 runs alone; both configurations
time out. BDD packages and tididi-cnf also receive a 60 GiB internal budget;
SDD has no separate internal setting. BDD allocations request transparent
huge pages. TDD circuit output uses dedicated gp3 storage. Contaminated timing
sources are rejected.

Other logic compilers use `r7i.24xlarge` (768 GiB RAM), nine concurrent
instances per compiler, 45 total. Two-minute counting uses the same machine
type with 24 instances at a time, one solver at a time. One-hour counting uses
eight instances per solver, 40 total. The 33 one-hour instances without a
previous completion by any solver are scored unsolved without another run.

Time covers process launch through exit: startup, parsing, compilation and
required output writing. TDD timing also includes normalization; the other
compilers' input preparation is outside timing. BDD/SDD diagrams need only
materialize in memory. Circuit checking is outside every compiler's clock.

## Logic compilers and counters

[Panini](https://github.com/meelgroup/KCBox),
[D4](https://github.com/crillab/d4v2),
[GPMC](https://github.com/System-Verification-Lab/GPMC) and
[c2d](http://reasoning.cs.ucla.edu/c2d/) 2.20 write circuits. GPMC uses d-DNNF
mode with variable elimination disabled to preserve all variables.
Counting uses the exact 2026 competition submissions and their track scripts.

| Compiler | Executable SHA-256 |
| --- | --- |
| tididi-cnf | `8c004f33f56bcca1adec461a0a4e74f6b1da19e93d4f7bb91e5b63073339d9d6` |
| Panini | `b209afacef898ca461ee37890dd069e3f36a9b2179d52ca6a17844ca279f26e1` |
| D4 | `24d5efb8e96896bf5c43d918f180773c6b771900ee57a152cd37834dc0eaddff` |
| GPMC | `c1c8c3e06811d907e74845b2602077ae4ec05323e517eb59bb0c88e4fcc182ce` |
| c2d | `ba2e7589f508847f1ce21d7310b39d011d6f4ed6b5cd8eff9cf015ba788f71b0` |

Source commits for Panini, D4 and GPMC are unavailable. The fingerprints
identify the measured binaries; the project links do not specify reproducible
source builds.

## Validation

Saved circuits must read back with complete variable coverage and agree with
available exact counts, including the compiler's reported count. D4 compilation
reports no count; the checker supplies it. An UNSAT answer is accepted as a
constant-false circuit. Disagreement with independent counts is a failure,
even if the circuit agrees with its compiler. Crashes and wrapper failures
remain unsolved. BDD/SDD packages must exit successfully with a nonnegative
final size; SDD must also report its node count and compilation footer.
Native fixed-width counts are not treated as exact evidence. These checks do
not establish full Boolean equivalence.
