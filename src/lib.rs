//! Count the models of a CNF formula by compiling it into a Tree Decision
//! Diagram.
//!
//! tididi-cnf answers the model count, the weighted model count and their
//! projections onto a set of variables: the model count and the projected
//! counts exactly, the weighted model count in a bounded-precision log domain.
//! It preprocesses the formula, chooses a vtree for what is left and compiles
//! it bottom-up, with fallbacks when a compile runs out of time or memory. It
//! is the library behind the `tididi` binary, and [`count`] makes the binary's
//! whole run on one instance.
//!
//! # Start here
//!
//! [`count`] takes an instance, the task, and the [`Limits`] to run under, and
//! returns an [`Answer`]: the verdict, the [`Count`] and the run's [`Stats`].
//! Here the instance is (x1 ∨ x2) ∧ (¬x1 ∨ x3) ∧ (¬x2 ∨ ¬x3), parsed from
//! DIMACS text with [`parse_dimacs`], and the task is its model count:
//!
//! ```
//! use tididi_cnf::{count, parse_dimacs, Limits, Mode};
//!
//! let formula = parse_dimacs("p cnf 3 3\n1 2 0\n-1 3 0\n-2 -3 0\n".as_bytes())?;
//! let answer = count(formula, Some(Mode::Mc), &Limits::default())?;
//! println!("Models: {}", answer.count());
//! # assert_eq!(answer.count(), &tididi_cnf::Count::Integer(2u32.into()));
//! # assert!(answer.satisfiable());
//! # Ok::<(), tididi_cnf::Error>(())
//! ```
//!
//! Output:
//!
//! ```text
//! Models: 2
//! ```
//!
//! [`count`] also takes a DIMACS file's path. With `None` for the mode, the
//! instance's header chooses the task, as it does for the binary.
//!
//! A program that builds its formula holds it as data rather than text:
//! [`formula_from_clauses`] takes the clauses as DIMACS integers, with a show
//! set and literal weights for the tasks that read them, and returns what
//! [`parse_dimacs`] returns for the same formula.
//!
//! ```
//! use tididi_cnf::{count, formula_from_clauses, Limits, Mode};
//!
//! let formula = formula_from_clauses(3, [[1, 2], [-1, 3], [-2, -3]], None, &[])?;
//! let answer = count(formula, Some(Mode::Mc), &Limits::default())?;
//! println!("Models: {}", answer.count());
//! # assert_eq!(answer.count(), &tididi_cnf::Count::Integer(2u32.into()));
//! # Ok::<(), tididi_cnf::Error>(())
//! ```
//!
//! Output:
//!
//! ```text
//! Models: 2
//! ```
//!
//! # Tasks and input
//!
//! The instance is DIMACS CNF. Its `c t mc|wmc|pmc|pwmc` line names the task
//! ([`Mode`]): the model count, the weighted count, and their projected forms,
//! with `c p weight` lines for the literal weights and a `c p show` line for
//! the variables projected onto; a formula that declares neither a task nor a
//! show set is compiled.
//! The [user guide](https://github.com/Tractables/tididi-cnf/blob/main/docs/getting-started.md)
//! describes the input format, the answers and the binary.
//!
//! # One step at a time
//!
//! Below [`count`] are the steps a compile is made of, for a caller that wants
//! the diagram and its own choices at each: [`parse_dimacs`] reads the
//! formula, [`vtree::build_vtree`] builds a vtree for it from a spec with its
//! [`vtree::VtreeOptions`], and [`compile::compile_cnf`] compiles it on that vtree
//! under its [`compile::CompileOptions`] into a [`tididi::Tdd`], the
//! [`diagram`](compile::Compiled::diagram) of what it returns, whose queries
//! the [`tididi`] crate provides. This path runs no preprocessing and no
//! fallback.
//!
//! ```
//! use std::fs::File;
//! use std::io::BufReader;
//!
//! use tididi_cnf::compile::{compile_cnf, CompileOptions};
//! use tididi_cnf::vtree::{build_vtree, VtreeOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Five queens on a 5×5 board, none attacking another.
//! let file = BufReader::new(File::open("tests/data/queens5.cnf")?);
//! let (formula, _header) = tididi_cnf::parse_dimacs(file)?;
//! let vtree = build_vtree(&formula, "portfolio", &VtreeOptions::default())?;
//! let diagram = vtree
//!     .context()
//!     .run(|engine| compile_cnf(engine, &formula, &vtree, CompileOptions::default()))?
//!     .diagram;
//! println!("{} placements", diagram.model_count()?);
//! # assert_eq!(diagram.model_count()?, 10u32.into());
//! # Ok(())
//! # }
//! ```
//!
//! Output:
//!
//! ```text
//! 10 placements
//! ```
//!
//! # Applications
//!
//! The [configuration example](guide::examples::catalog) counts a
//! product's configurations, projects them onto some of the options, and
//! queries the compiled rules as a customer chooses.
//! The [reliability example](guide::examples::reliability) weighs links by
//! their probabilities to find how likely two sites stay connected.
//! The [circuit example](guide::examples::circuits) counts the distinct
//! products a multiplier outputs.
//! The [vtree example](guide::examples::vtree_choice) compiles one formula on
//! several vtrees and compares the circuits.
//!
//! The [guide] holds these worked examples and the architecture reference.
//!
//! # Limits
//!
//! [`Limits`] are the binary's `--time-limit-ms` and `--mem-limit-mb`. The
//! time limit bounds the whole run: every stage gets a share of it, and a run
//! that reaches it returns [`Error::Incomplete`], never a partial count. The
//! memory limit lowers the process's address-space limit while the run lasts,
//! and a model count that is refused memory retries on smaller pieces of the
//! formula; the call puts the limit back when it returns. The default sets
//! neither. A run that nears the address-space limit switches the allocator to
//! handing freed pages back to the system at once, and the process keeps that
//! setting after the run returns.
//!
//! Calls from several threads run one at a time: a [`count`] call waits for
//! the one in progress to return. [`count`] prints its progress to stderr, as
//! the binary does.
//!
//! # Platform
//!
//! Linux, and WebAssembly through Emscripten (`wasm32-unknown-emscripten`).
//! Preprocessing and vtree construction come from the
//! [vitri](https://github.com/Tractables/vitri) crate, which builds CaDiCaL,
//! Arjun and their dependencies from source and links them natively; its
//! [building guide](https://github.com/Tractables/vitri/blob/main/docs/building.md)
//! lists the C++ toolchain it needs, for either target. Natively the crate
//! installs jemalloc as the global allocator, so a program that links it
//! cannot choose its own, and it exports jemalloc's startup configuration,
//! which then holds for the whole process. Recovering from a refused
//! allocation needs `panic = "unwind"`, the default. WebAssembly keeps the
//! target's own allocator and has no address-space limit to lower, so it
//! refuses [`Limits::mem_limit_mb`].
//!
//! # Module reference
//!
//! - The crate root: [`count`] and what it takes and returns ([`Input`],
//!   [`Mode`], [`Limits`], [`Answer`], [`Count`], [`Stats`], [`Error`]), and
//!   the two ways a formula comes in, [`parse_dimacs`] and
//!   [`formula_from_clauses`].
//! - [`vtree`]: vtree construction from a spec, whole or one tree per
//!   independent component.
//! - [`compile`]: the compiler, [`compile::compile_cnf`], and what it takes and
//!   returns.
//! - [`tididi`]: the diagram library, re-exported at the version this crate
//!   compiles against.
//! - [`vitri`]: the preprocessing and vtree library, re-exported for the
//!   formula types [`parse_dimacs`] and [`formula_from_clauses`] return.
//! - [`guide`]: the worked examples and the architecture reference.

#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]
#![warn(unreachable_pub)]

pub mod compile;
pub mod guide;
pub mod vtree;

mod budget;
mod cli;
mod config;
mod convert;
mod driver;
mod enumeration;
mod error;
mod formula;
mod frontend;
#[cfg_attr(target_family = "wasm", path = "mem_wasm.rs")]
mod mem;
mod output;
mod problem;
mod run;
mod stage;

#[cfg(test)]
#[path = "tests/support.rs"]
pub(crate) mod test_support;

/// One mebibyte, the unit memory limits are given in and the memory reports
/// print in. Here rather than in `mem`, whose WebAssembly stand-in the reports
/// build against too.
const MIB: u64 = 1024 * 1024;

/// The diagram library this crate compiles into: vtrees, diagrams, the engine
/// and its queries.
pub use tididi;

/// The library this crate reads formulas and builds vtrees with. Its
/// [`CnfFormula`](vitri::cnf::CnfFormula) and [`CnfMeta`](vitri::cnf::CnfMeta)
/// are the formula types [`parse_dimacs`] and [`formula_from_clauses`] return
/// and [`compile`] takes.
pub use vitri;

pub use config::{Limits, Mode};
pub use error::Error;
pub use formula::{formula_from_clauses, parse_dimacs};
pub use output::Stats;
pub use run::{Answer, Count, Input, count};

#[doc(hidden)]
pub use cli::run_cli;
