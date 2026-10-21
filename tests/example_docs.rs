//! Keep the worked examples' excerpts tied to the programs they come from, and
//! the README's library example tied to the crate's tested one.
//! `.github/scripts/check_docs.py` runs those programs and compares their output.

/// Compare Rust excerpts without indentation introduced by their surrounding scope.
fn normalized(source: &str) -> String {
    source.lines().map(str::trim).collect::<Vec<_>>().join("\n")
}

/// Every worked example with the program of the same name, which its
/// "complete program" link must name too.
fn walkthroughs() -> Vec<(String, String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root.join("docs/examples")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let markdown = std::fs::read_to_string(&path).unwrap();
        let example = format!("{name}.rs");
        assert!(markdown.contains(&format!("/examples/{example})")), "{name}: the complete-program link names {example}");
        let source = std::fs::read_to_string(root.join("examples").join(&example)).unwrap_or_else(|e| panic!("{name}: {example}: {e}"));
        out.push((name, markdown, source));
    }
    assert!(!out.is_empty(), "no worked examples found");
    out
}

#[test]
fn walkthrough_code_comes_from_the_runnable_examples() {
    for (name, markdown, source) in walkthroughs() {
        let source = format!("\n{}\n", normalized(&source));
        let mut excerpts = 0;
        let mut lines = markdown.lines();
        while let Some(line) = lines.next() {
            let Some(attributes) = line.strip_prefix("```") else { continue };
            if !attributes.starts_with("rust") && !attributes.contains("tested-example") {
                continue;
            }
            assert_eq!(line, "```rust,ignore,{class=tested-example}", "{name}: excerpts are checked against the executable example");
            let mut snippet = Vec::new();
            let mut closed = false;
            for line in lines.by_ref() {
                if line == "```" {
                    closed = true;
                    break;
                }
                snippet.push(line);
            }
            assert!(closed, "{name}: unclosed Rust excerpt");
            let snippet = normalized(&snippet.join("\n"));
            assert!(!snippet.is_empty(), "{name}: empty Rust excerpt");
            // Whole lines only: an excerpt starts and ends at line boundaries of the program.
            assert!(source.contains(&format!("\n{snippet}\n")), "{name}: excerpt does not occur in the runnable example:\n{snippet}");
            excerpts += 1;
        }
        assert!(excerpts > 0, "{name}: worked example has no checked excerpts");
    }
}

/// The README shows the crate example without its hidden error-handling lines.
#[test]
fn readme_example_matches_the_tested_crate_example() {
    let readme = normalized(include_str!("../README.md"));
    let (_, example) = readme.split_once("```rust\n").expect("README Rust example");
    let (example, rest) = example.split_once("\n```").expect("closed README example");
    assert!(!rest.contains("```rust"), "check every README Rust example");
    let docs = include_str!("../src/lib.rs").lines()
        .filter_map(|line| line.strip_prefix("//!"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>().join("\n");
    let (_, tested) = docs.split_once("```\n").expect("crate doctest");
    let (tested, _) = tested.split_once("\n```").expect("closed crate doctest");
    let visible = tested.lines().filter(|line| !line.starts_with("# "))
        .collect::<Vec<_>>().join("\n");
    assert_eq!(normalized(example), normalized(&visible), "README must match the executed doctest");
}
