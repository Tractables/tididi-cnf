//! The one error type the library returns and the `tididi` binary reports.

/// Everything that stops a run before it has an answer.
///
/// The variant is the failure class, which fixes the status the binary exits
/// with and the name the run record gives it ([`Error::kind`]). The string
/// says what went wrong, in words an operator can act on;
/// [`Display`](std::fmt::Display) prints it, and the binary prints it on
/// stderr after `error: `.
#[derive(Debug)]
pub enum Error {
    /// An option is contradictory, or inert in the mode this run resolved to.
    Usage(String),
    /// An environment setting is invalid, or names a control that was retired.
    Environment(String),
    /// A preprocessing frontend or a compile stage failed.
    Stage(String),
    /// The requested vtree spec cannot serve this run.
    VtreeSpec(String),
    /// The input could not be opened or read, or does not describe a formula.
    Input(String),
    /// An artifact the run was asked to write could not be produced.
    Artifact(String),
    /// The run ended without a count: it reached its time limit, or every
    /// compile and recovery path gave up. The stage that gave up says on
    /// stderr what stopped it; the message says which of these ended the run.
    Incomplete(String),
}

impl Error {
    /// An allocation was refused and the run has no count to print.
    pub(crate) fn out_of_memory() -> Self {
        Error::Incomplete(OUT_OF_MEMORY.to_string())
    }

    /// The compile, and every path behind it, gave up without a count.
    pub(crate) fn compile_gave_up() -> Self {
        Error::Incomplete("no answer: the compile gave up".to_owned())
    }

    /// Every stage of a projected count declined or gave up.
    pub(crate) fn every_stage_gave_up() -> Self {
        Error::Incomplete("no answer: every stage gave up".to_owned())
    }

    /// `map_err` adapter for a failed preprocessing or expansion stage:
    /// `{what} failed: {error}`.
    pub(crate) fn stage_failed<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> Self {
        move |error| Error::Stage(format!("{what} failed: {error}"))
    }

    /// The class's name, as the run record's `exit_reason` gives it: `usage`,
    /// `environment`, `stage`, `vtree-spec`, `input`, `artifact` or
    /// `incomplete`.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Usage(_) => "usage",
            Error::Environment(_) => "environment",
            Error::Stage(_) => "stage",
            Error::VtreeSpec(_) => "vtree-spec",
            Error::Input(_) => "input",
            Error::Artifact(_) => "artifact",
            Error::Incomplete(_) => "incomplete",
        }
    }

    /// The status the binary exits with for this class.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Error::Usage(_) | Error::Environment(_) | Error::Stage(_) => 2,
            Error::VtreeSpec(_)
            | Error::Input(_)
            | Error::Artifact(_)
            | Error::Incomplete(_) => 1,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Error::Usage(message)
            | Error::Environment(message)
            | Error::Stage(message)
            | Error::VtreeSpec(message)
            | Error::Input(message)
            | Error::Artifact(message)
            | Error::Incomplete(message) => message,
        };
        f.write_str(message)
    }
}

impl std::error::Error for Error {}

/// Stderr diagnostic for a run an allocation refusal ended.
///
/// A benchmark harness reads a process as out of memory by finding
/// `"memory allocation of"` and `"bytes failed"` in its stderr, which is what
/// the allocator prints when it aborts. A compile that refuses gracefully never
/// reaches the allocator's own message, so it says the same thing here and the
/// run is attributed the same way.
///
/// The compile raises it when reducing the finished diagram is refused an
/// allocation (`compile::GiveUp::OutOfMemory`).
const OUT_OF_MEMORY: &str = "memory allocation of unknown bytes failed while reducing \
     the compiled diagram: the run reached its memory limit or the system ran out of \
     memory, and has no answer. A larger memory limit gives it more room.";

#[cfg(test)]
#[path = "tests/error.rs"]
mod tests;
