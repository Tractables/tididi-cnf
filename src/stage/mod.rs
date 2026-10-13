//! The counting pipelines the driver dispatches to, one module per mode, and
//! what they share.
//!
//! `mc/` is the model count: Arjun reduces the formula, the primary compile
//! runs on the reduction, and the fallbacks and Shannon split follow it; under
//! an explicit `--vtree` the streaming compile runs instead.
//! `wmc.rs` is the weighted count, `pmc.rs` the projected count and `pwmc.rs`
//! the weighted projected count. `projected.rs` is what the two projected
//! counts share: the stage seam they are written on, their common stages and
//! their counting vtree. `conditioning.rs` is the projected count's
//! show-variable conditioning and `structured_count.rs` its count over a
//! separator. Every stage reads the run through [`Run`], a leaf compile that
//! may panic runs under [`run_isolated`], and the model count and the
//! weighted projected count share [`count_streaming`]. All of them call
//! `compile`, `vtree` and `enumeration`; nothing below them calls back up
//! here.

use std::time::Instant;

use num_bigint::BigUint;
use tididi::Engine;
use tididi::vtree::VarId;
use vitri::cnf::CnfFormula;

use crate::budget::vtree_deadline;
use crate::compile::{CompileRun, model_count_streaming};
use crate::config::Config;
use crate::vtree::{VtreeOptions, build_vtree, built_or_give_up, split_components};

mod conditioning;
mod mc;
mod pmc;
mod projected;
mod pwmc;
mod structured_count;
mod wmc;

pub(crate) use mc::{run_mc_count, run_mc_streaming};
pub(crate) use pmc::run_pmc_count;
pub(crate) use pwmc::run_pwmc_count;
pub(crate) use wmc::run_wmc_count;

/// The vtree spec a stage builds its own trees on, wherever it does not take
/// the run's: vitri's portfolio. The model count's unreduced retry and Shannon
/// cofactors, PMC's shatter components and conditioning branches, and the
/// weighted count's reduced attempt build on it.
const PORTFOLIO_VTREE_SPEC: &str = "portfolio";

/// What every stage of one run reads: the engine it compiles on, the resolved
/// configuration, the run's wall, and what the run sets for each compile. The
/// driver builds it once; a stage that needs more is handed it beside the run.
#[derive(Clone, Copy)]
pub(crate) struct Run<'a> {
    pub(crate) eng: &'a Engine,
    pub(crate) config: &'a Config,
    /// The run's wall, `None` when it has none.
    pub(crate) deadline: Option<Instant>,
    pub(crate) compile: CompileRun<'a>,
}

/// Count `formula` by the streaming compile over its components, as
/// `vtree::split_components` gives them for the run's spec, one component at
/// a time, forgetting `forget` as [`model_count_streaming`] does, under what
/// the run sets for each compile; `None` if a component's vtree could not be
/// built or the compile gave up. Returns the count and the compile's peak
/// size.
///
/// Each component's vtree is built as the compile reaches it:
/// `minfill-primal` for a component of at most 30 variables, the run's spec
/// otherwise. Every component shares one construction deadline, derived from
/// the run's wall now; deriving it per component would leave the total
/// unbounded. Components build one at a time, so an early one may spend it
/// all and later ones fall back to vitri's last-resort build.
fn count_streaming(run: &Run<'_>, formula: &CnfFormula, forget: Option<&[VarId]>) -> Option<(BigUint, usize)> {
    let components = split_components(formula, run.config.vtree_spec());
    let options = VtreeOptions::default()
        .with_deadline(vtree_deadline(run.deadline))
        .with_run_policy(run.config);
    let build = |component: &CnfFormula| {
        let spec = if component.num_vars() <= 30 { "minfill-primal" } else { run.config.vtree_spec() };
        built_or_give_up(run.config.vtree_time.time(|| build_vtree(component, spec, &options)))
    };
    model_count_streaming(run.eng, formula, &components, build, forget, run.deadline, run.compile)
}

/// Best-effort extraction of a human-readable message from a panic
/// payload. Rust panics carry the message as either `&'static str`
/// (panic with a string literal) or `String` (panic with a formatted
/// message). For other payload types we fall back to a placeholder.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Run a leaf compile under `catch_unwind` panic isolation: on a panic (a
/// refused allocation among them, which unwinds as `crate::mem` says), log the
/// payload after `label`, the `[<mode>] <stage>` it ran as, and return `None`,
/// so the panic becomes an ordinary give-up — the next cascade stage runs, or
/// the caller scores the instance unsolved — instead of aborting the process.
///
/// The shared panic-isolation seam for leaf compiles: the model count's
/// compile attempts, the WMC compiles, PMC's first compile and the PWMC
/// compile route through it. The model count's recovery catches its own
/// panics, because it does more on one: it splits the formula.
fn run_isolated<T>(label: impl std::fmt::Display, f: impl FnOnce() -> Option<T>) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = panic_payload_message(payload.as_ref());
            eprintln!("{label}: panicked: {msg}");
            None
        }
    }
}
