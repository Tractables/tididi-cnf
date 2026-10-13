use super::*;

/// The out-of-memory diagnostic carries the allocator's own words, which a
/// harness reads to classify the run, and says what happened in plain
/// words, naming neither an internal path nor a command-line flag.
#[test]
fn out_of_memory_reads_as_the_allocators_failure() {
    let error = Error::out_of_memory();
    let message = error.to_string();
    assert!(message.contains("memory allocation of") && message.contains("bytes failed"), "{message}");
    assert!(!message.contains("::"), "names an internal path: {message}");
    assert!(message.contains("memory limit") && !message.contains("--"), "{message}");
    assert_eq!(error.exit_code(), 1);
}
