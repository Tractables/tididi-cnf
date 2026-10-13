//! CLI entry point for the `tididi` binary.
//!
//! The run is `tididi_bonsai::run_cli`; this prints its error, if any,
//! and exits with the error's status.

fn main() {
    let Err(error) = tididi_bonsai::run_cli() else {
        return;
    };
    eprintln!("error: {error}");
    std::process::exit(error.exit_code());
}
