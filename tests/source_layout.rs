//! Source layout and comment checks.
//!
//! Six checks. The first five read the crate's Rust sources, the files under
//! `src/` and the integration tests under `tests/`; the sixth reads every file
//! under `examples/` and `tests/`. Rule 1 reads the production files, the
//! ones under `src/` that are not themselves test modules; rules 2, 4 and 5
//! read every file, since a test's comments are prose like any other, point a
//! reader at the same files and name the same items. The prose rules read the
//! comment lines (`//`, `///`, `//!`) and the messages of `unreachable!`,
//! `panic!` and `assert*!` invocations, which a reader meets in the same way
//! as a comment.
//!
//! 1. A production file carries no `#[cfg(test)]` item other than the module
//!    declaration for its test file and the imports it needs. A module's unit
//!    tests are a child module in `<dir>/tests/<stem>.rs` beside it (for a
//!    `mod.rs`, `<dir>/tests/mod.rs`), and the helpers the unit tests share
//!    are in `src/tests/support.rs`.
//! 2. A cited `<file>.rs` path names a file that exists. A citation is
//!    read wherever it appears, backticked spans included, and a cited
//!    `<dir>/<file>.rs` must match that much of a real path under `src/` and not
//!    merely the file name, or name a file relative to the repository root.
//! 3. Every `pub mod` in `src/lib.rs` has a row in the module table in
//!    `docs/architecture.md`.
//! 4. No all-caps word in prose. Emphasis is carried by sentence structure;
//!    a name that is genuinely upper case is a code item and belongs in
//!    backticks, which this check strips before looking.
//! 5. A crate item whose name carries an underscore is backticked where a
//!    comment names it.
//! 6. Every input a test or a doctest reads lives under `tests/data/`: the
//!    rest of `tests/`, and `examples/`, hold Rust sources only.
//!
//! A fenced block inside a doc comment is source the reader compiles, so the
//! prose checks skip from one fence line to the next.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// Upper-case words that are the ordinary spelling of the thing they name.
const ACRONYMS: &[&str] = &[
    "API", "ASCII", "BCP", "BVE", "CDCL", "CLI", "CNF", "CPU", "DIMACS", "DVE", "IJCAI", "ISCAS",
    "JSON", "LCA", "LCG", "MCC", "MIB", "MMIX", "OOM", "PHP", "PMC", "POD", "POSIX", "PRNG", "PWMC",
    "RAII", "README", "RLIMIT", "SAT", "SBVA", "TDD", "UNSAT", "URL", "VSZ", "WMC", "XOR",
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A file whose contents are themselves tests, and so outside the production
/// rules: an integration test, a unit-test module, or the shared test support.
fn is_test_file(rel: &str) -> bool {
    rel.starts_with("tests/") || rel.contains("/tests/")
}

#[test]
fn test_files_are_recognized_at_the_crate_root_and_within_modules() {
    for rel in [
        "tests/protocol.rs",
        "tests/common/mod.rs",
        "src/tests/support.rs",
        "src/compile/tests/step.rs",
        "src/compile/sat_prune/tests/mod.rs",
    ] {
        assert!(is_test_file(rel), "{rel}");
    }
    for rel in ["src/lib.rs", "src/compile/step.rs", "src/stage/mc/windows.rs"] {
        assert!(!is_test_file(rel), "{rel}");
    }
}

/// Every file under the directories `dirs` of the repository root, as a path
/// relative to that root, sorted.
fn files_under(dirs: &[&str]) -> Vec<(String, PathBuf)> {
    let root = crate_dir();
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = dirs.iter().map(|dir| root.join(dir)).collect();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("the directory is readable") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(&root)
                    .expect("under the repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    out.sort();
    out
}

/// Every `.rs` file under `src/` and `tests/`, as a path relative to the
/// repository root, sorted.
fn source_files() -> Vec<(String, PathBuf)> {
    files_under(&["src", "tests"]).into_iter().filter(|(rel, _)| rel.ends_with(".rs")).collect()
}

#[test]
fn inputs_live_under_tests_data() {
    let misplaced: Vec<String> = files_under(&["examples", "tests"])
        .into_iter()
        .map(|(rel, _)| rel)
        .filter(|rel| !rel.ends_with(".rs") && !rel.starts_with("tests/data/"))
        .collect();
    assert!(misplaced.is_empty(), "files outside tests/data/ that are not Rust sources:\n{}", misplaced.join("\n"));
}

fn production_sources() -> Vec<(String, PathBuf)> {
    source_files().into_iter().filter(|(rel, _)| !is_test_file(rel)).collect()
}

/// What a rule does with a backticked span: a prose rule reads the span as
/// the code it is and drops it; the citation rule reads the path inside it.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Code {
    Strip,
    Keep,
}

/// `text` with every backticked span dropped or kept, per `code`. The
/// backticks themselves always become spaces, so a span never runs into the
/// word beside it.
fn code_spans(text: &str, code: Code) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for part in text.split('`') {
        if !in_code || code == Code::Keep {
            out.push_str(part);
        }
        out.push(' ');
        in_code = !in_code;
    }
    out
}

/// The comment body of a line, or `None` if the line is not a comment. The
/// leading marker is removed and backticked spans are handled per `code`.
fn comment_prose(line: &str, code: Code) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("//") {
        return None;
    }
    let body = trimmed.trim_start_matches('/').trim_start_matches('!');
    Some(code_spans(body, code))
}

/// The maximal runs of three or more upper-case letters in `prose`.
fn all_caps_words(prose: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    for ch in prose.chars() {
        if ch.is_ascii_uppercase() {
            run.push(ch);
        } else {
            if run.len() >= 3 {
                out.push(std::mem::take(&mut run));
            } else {
                run.clear();
            }
        }
    }
    if run.len() >= 3 {
        out.push(run);
    }
    out
}

/// The `<file>.rs` paths cited in `prose`, with any URL removed first. A
/// citation may name directories (`compile/sat_prune/mod.rs`); a leading
/// `src/` is dropped, since that is where the crate's sources are.
fn cited_paths(prose: &str) -> Vec<String> {
    let without_urls: String = prose
        .split_whitespace()
        .filter(|w| !w.contains("://"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = Vec::new();
    for word in
        without_urls.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/'))
    {
        let name = word.trim_matches(['.', '/']);
        if !name.ends_with(".rs") {
            continue;
        }
        let stem = &name[..name.len() - 3];
        let segment_ok = |s: &str| {
            !s.is_empty()
                && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        };
        if stem.split('/').all(segment_ok) {
            out.push(name.strip_prefix("src/").unwrap_or(name).to_string());
        }
    }
    out
}

#[test]
fn citations_are_read_whole_and_relative_to_src() {
    assert_eq!(
        cited_paths("see `src/compile/step.rs` and tests/protocol.rs."),
        vec!["compile/step.rs", "tests/protocol.rs"],
    );
    assert_eq!(cited_paths("https://example.org/a.rs and Foo.rs"), Vec::<String>::new());
}

/// The prose of every comment line of `text`, as `(line number, prose)`. A
/// fenced block of a doc comment is source rather than prose, so the fence
/// lines and everything between them are left out.
fn prose_lines(text: &str, code: Code) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fenced = false;
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") {
            continue;
        }
        let body = trimmed.trim_start_matches('/').trim_start_matches('!');
        if body.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some(prose) = comment_prose(line, code) {
            out.push((n + 1, prose));
        }
    }
    out
}

/// The macros whose string arguments are a message the reader reads as prose.
const MESSAGE_MACROS: &[&str] =
    &["unreachable", "panic", "assert", "assert_eq", "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne"];

/// The name of a message macro invoked at `i`, with the index of the character
/// after its opening delimiter. `None` if no invocation starts here.
fn message_macro_at(chars: &[char], i: usize) -> Option<usize> {
    if i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_') {
        return None;
    }
    let name = MESSAGE_MACROS.iter().find(|name| {
        let end = i + name.chars().count();
        chars.get(i..end).is_some_and(|w| w.iter().copied().eq(name.chars()))
            && chars.get(end) == Some(&'!')
    })?;
    let mut j = i + name.chars().count() + 1;
    while chars.get(j).is_some_and(|c| c.is_whitespace()) {
        j += 1;
    }
    matches!(chars.get(j), Some('(' | '[' | '{')).then_some(j + 1)
}

/// The contents of the string literal opening at `i`, with the index just past
/// its closing quote. Escapes are unwrapped so the prose reads as it prints.
fn read_string(chars: &[char], i: usize, line: &mut usize) -> (String, usize) {
    let mut out = String::new();
    let mut j = i + 1;
    while j < chars.len() && chars[j] != '"' {
        if chars[j] == '\\' {
            j += 1;
            match chars.get(j) {
                // A continuation swallows the newline and the next line's indent.
                Some('\n') => {
                    *line += 1;
                    j += 1;
                    while chars.get(j).is_some_and(|c| c.is_whitespace() && *c != '\n') {
                        j += 1;
                    }
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                    continue;
                }
                Some(c) => out.push(*c),
                None => break,
            }
        } else {
            if chars[j] == '\n' {
                *line += 1;
            }
            out.push(chars[j]);
        }
        j += 1;
    }
    (out, j + 1)
}

/// The prose of every panic message in `text`, as `(line number, prose)`: the
/// string literals directly in a message macro's argument list, backticked
/// spans removed the same way a comment's are. A literal nested deeper, in a
/// call or a pattern, is a value the check computes with, not its message.
fn message_prose(text: &str, code: Code) -> Vec<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut line = 1usize;
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                line += usize::from(chars[i] == '\n');
                i += 1;
            }
            i = (i + 2).min(chars.len());
        } else if c == '"' {
            let at = line;
            let (lit, next) = read_string(&chars, i, &mut line);
            if depth == 1 {
                out.push((at, code_spans(&lit, code)));
            }
            i = next;
        } else if c == '\'' && (chars.get(i + 1) == Some(&'\\') || chars.get(i + 2) == Some(&'\'')) {
            // A character literal, not a lifetime: skip past its closing quote.
            i += 2;
            while i < chars.len() && chars[i] != '\'' {
                i += usize::from(chars[i] == '\\') + 1;
            }
            i += 1;
        } else if depth > 0 {
            depth += usize::from(matches!(c, '(' | '[' | '{'));
            depth -= usize::from(matches!(c, ')' | ']' | '}'));
            i += 1;
        } else if let Some(after) = message_macro_at(&chars, i) {
            depth = 1;
            i = after;
        } else {
            i += 1;
        }
    }
    out
}

/// Every line of `text` a reader reads as prose: its comments and its panic
/// messages.
fn file_prose(text: &str, code: Code) -> Vec<(usize, String)> {
    let mut out = prose_lines(text, code);
    out.extend(message_prose(text, code));
    out
}

#[test]
fn prose_carries_emphasis_by_structure_not_by_capitals() {
    let acronyms: HashSet<&str> = ACRONYMS.iter().copied().collect();
    let mut hits: Vec<String> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (rel, path) in source_files() {
        let text = fs::read_to_string(&path).expect("a readable source file");
        for (n, prose) in file_prose(&text, Code::Strip) {
            for word in all_caps_words(&prose) {
                if acronyms.contains(word.as_str()) {
                    continue;
                }
                if seen.insert((rel.clone(), word.clone())) {
                    hits.push(format!("{rel}:{n}: {word}"));
                }
            }
        }
    }
    assert!(hits.is_empty(), "all-caps words in prose:\n{}", hits.join("\n"));
}

/// Every item the crate declares whose name carries an underscore.
///
/// Those are the names a global identifier rename rewrites and that no English
/// sentence contains by accident, which is the pair of properties the rule
/// below needs. Single-word names are left out: `new`, `count`, `run`, `solve`
/// and `compile` are all items here and all ordinary words.
fn underscored_item_names() -> HashSet<String> {
    const DECLARES: &[&str] = &["fn", "struct", "enum", "trait", "const", "static", "type", "mod", "union"];
    let mut names = HashSet::new();
    for (_, path) in source_files() {
        let text = fs::read_to_string(&path).expect("a readable source file");
        for line in text.lines() {
            let mut words = line.split_whitespace().peekable();
            while let Some(word) = words.next() {
                if !DECLARES.contains(&word) {
                    continue;
                }
                let Some(next) = words.peek() else { continue };
                let name: String = next
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                // `const _` names nothing, so a bare `_` is not an item.
                if name.contains('_') && name.chars().any(|c| c.is_ascii_alphanumeric()) {
                    names.insert(name);
                }
            }
        }
    }
    names
}

/// `prose` with every Markdown link target blanked out, keeping the offsets.
///
/// A link target is a path, not prose, and it is spelled without backticks
/// because that is what rustdoc resolves. Blanked: the `(..)` of an inline
/// link, the `[..]` of a shortcut link, and the target of a reference
/// definition.
fn without_link_targets(prose: &str) -> String {
    let mut out: Vec<char> = prose.chars().collect();
    let blank = |from: usize, to: usize, out: &mut Vec<char>| {
        for c in &mut out[from..to] {
            *c = ' ';
        }
    };
    let n = out.len();
    let mut i = 0;
    while i < n {
        match out[i] {
            '[' => {
                let end = (i..n).find(|&j| out[j] == ']').unwrap_or(n - 1);
                blank(i, end.min(n), &mut out);
                // `[label]: target` — the rest of the line is the target.
                if out.get(end + 1) == Some(&':') {
                    blank(end, n, &mut out);
                    break;
                }
                i = end;
            }
            '(' if i > 0 && out[i - 1] == ']' => {
                let end = (i..n).find(|&j| out[j] == ')').map_or(n, |j| j + 1);
                blank(i, end, &mut out);
                i = end;
            }
            _ => i += 1,
        }
    }
    out.into_iter().collect()
}

/// Each `[A-Za-z0-9_]+` run in `prose`, with its byte offset.
fn words_with_offsets(prose: &str) -> Vec<(&str, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in prose.char_indices() {
        let part = c.is_ascii_alphanumeric() || c == '_';
        match (part, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((&prose[s..i], s));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((&prose[s..], s));
    }
    out
}

/// A crate item named in a comment belongs in backticks.
///
/// A global identifier rename reaches English prose and leaves a function
/// name where a verb belonged, because a bare name in a comment is
/// indistinguishable from a word. Backticks make the two distinguishable, so
/// the same rename cannot do it; they are also how rustdoc renders a name.
/// Test files are in scope.
#[test]
fn a_comment_backticks_the_crate_items_it_names() {
    let items = underscored_item_names();
    let mut hits: Vec<String> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (rel, path) in source_files() {
        let text = fs::read_to_string(&path).expect("a readable source file");
        for (n, prose) in prose_lines(&text, Code::Strip) {
            let prose = without_link_targets(&prose);
            for (word, at) in words_with_offsets(&prose) {
                // A path component is not a name the reader mistakes for a
                // word: `compile/sat_prune/mod.rs` is a citation, and the
                // citation rule reads it whole.
                let bytes = prose.as_bytes();
                let before = at.checked_sub(1).map(|i| bytes[i]);
                let after = bytes.get(at + word.len()).copied();
                if before == Some(b'/') || after == Some(b'/') {
                    continue;
                }
                if items.contains(word) && seen.insert((rel.clone(), word.to_string())) {
                    hits.push(format!("{rel}:{n}: {word}"));
                }
            }
        }
    }
    assert!(hits.is_empty(), "crate items named in prose without backticks:\n{}", hits.join("\n"));
}

#[test]
fn a_comment_cites_only_a_file_that_exists() {
    // Every source path relative to `src/`, plus the `<dir>.rs` spelling of a
    // `<dir>/mod.rs`, which is how a module is named in prose.
    let mut known: Vec<String> = Vec::new();
    for (rel, _) in source_files() {
        let Some(rel) = rel.strip_prefix("src/") else { continue };
        known.push(rel.to_string());
        if let Some(dir) = rel.strip_suffix("/mod.rs") {
            known.push(format!("{dir}.rs"));
        }
    }
    // A citation names a suffix of a real path under `src/`, at whole path
    // segments, or a file relative to the repository root.
    let root = crate_dir();
    let resolves = |cited: &str| {
        known.iter().any(|k| k == cited || k.ends_with(&format!("/{cited}")))
            || root.join(cited).is_file()
    };
    let mut hits: Vec<String> = Vec::new();
    for (rel, path) in source_files() {
        let text = fs::read_to_string(&path).expect("a readable source file");
        for (n, prose) in file_prose(&text, Code::Keep) {
            for cited in cited_paths(&prose) {
                if resolves(&cited) {
                    continue;
                }
                hits.push(format!("{rel}:{n}: cites {cited}, which does not exist"));
            }
        }
    }
    assert!(hits.is_empty(), "comments citing a file that does not exist:\n{}", hits.join("\n"));
}

/// The item a `#[cfg(test)]` guards, excluding fields and conditional hook calls.
fn guarded_item(lines: &[&str], at: usize) -> Option<String> {
    let decl = lines[at + 1..]
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with("#["))?;
    if decl.starts_with("use ") {
        return None;
    }
    let words: Vec<&str> = decl.split_whitespace().collect();
    if words.contains(&"mod") && decl.ends_with(';') {
        return None;
    }
    for (i, w) in words.iter().enumerate() {
        if matches!(*w, "fn" | "struct" | "enum" | "const" | "static" | "impl" | "trait" | "type")
            && let Some(next) = words.get(i + 1)
        {
            let name: String =
                next.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    if decl.starts_with("if ") {
        return None;
    }
    if let Some((field, _)) = decl.split_once(':')
        && !field.is_empty()
        && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(decl.chars().take(40).collect())
}

#[test]
fn cfg_scan_distinguishes_hook_fields_and_calls_from_test_items() {
    for decl in [
        "refuse_after: Cell<Option<u32>>,",
        "refuse_after: Cell::new(None),",
        "if self.refuses_reserve() { return Err(Error::Incomplete(String::new())); }",
        "mod tests;",
        "pub(crate) mod test_support;",
        "use crate::test_support::formula;",
    ] {
        assert_eq!(guarded_item(&["#[cfg(test)]", decl], 0), None);
    }
    for decl in [
        "pub(crate) fn helper() {}",
        "struct Helper;",
        "impl Config {",
        "const ALL: [AnsweredBy; 6] = [",
        "mod tests {",
    ] {
        assert!(guarded_item(&["#[cfg(test)]", decl], 0).is_some());
    }
}

#[test]
fn a_production_file_holds_no_test_only_item() {
    let mut hits: Vec<String> = Vec::new();
    for (rel, path) in production_sources() {
        let text = fs::read_to_string(&path).expect("a readable source file");
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            if !line.trim().starts_with("#[cfg(test)]") {
                continue;
            }
            let Some(item) = guarded_item(&lines, n) else { continue };
            hits.push(format!("{rel}:{}: test-only item {item}", n + 1));
        }
    }
    assert!(hits.is_empty(), "test-only items in production files:\n{}", hits.join("\n"));
}

/// Whether one row of a Markdown table names the module `name` in its first
/// cell: as `` `<name>` ``, or by its source, `` `src/<name>.rs` `` or
/// `` `src/<name>/` ``. A row may name several modules.
fn row_names_module(row: &str, name: &str) -> bool {
    let Some(first) = row.trim_start().strip_prefix('|').and_then(|r| r.split('|').next()) else {
        return false;
    };
    [format!("`{name}`"), format!("`src/{name}.rs`"), format!("`src/{name}/`")]
        .iter()
        .any(|spelling| first.contains(spelling.as_str()))
}

#[test]
fn a_module_row_is_read_from_its_first_cell() {
    assert!(row_names_module("| `src/vtree.rs` | Vtree construction. |", "vtree"));
    assert!(row_names_module("| `src/budget.rs`, `src/mem.rs` | The hooks. |", "mem"));
    assert!(row_names_module("| `src/compile/` | The compile. |", "compile"));
    assert!(row_names_module("| `vtree` | Vtree construction. |", "vtree"));
    assert!(!row_names_module("| `src/run.rs` | Calls `src/vtree.rs`. |", "vtree"));
    assert!(!row_names_module("| `src/tests/vtree.rs` | Its tests. |", "vtree"));
}

#[test]
fn every_public_module_has_a_row_in_the_module_table() {
    // The table's module names may be links, so the brackets come out before
    // a row is matched.
    let table = fs::read_to_string(crate_dir().join("docs/architecture.md"))
        .expect("the architecture document is readable")
        .replace(['[', ']'], "");
    let lib = fs::read_to_string(crate_dir().join("src/lib.rs")).expect("lib.rs is readable");
    let mut missing: Vec<String> = Vec::new();
    for line in lib.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("pub mod ") else { continue };
        let name: String =
            rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            continue;
        }
        if !table.lines().any(|row| row_names_module(row, &name)) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "public modules with no row in docs/architecture.md: {}",
        missing.join(", ")
    );
}

/// A guard on the message scan: it reads the message of a panic macro, follows
/// a continuation onto the next line, and reads nothing else — not a plain
/// call, not a comment that names one of the macros, and not a value nested
/// in the macro's arguments.
#[test]
fn the_message_scan_reads_panic_messages_and_nothing_else() {
    let text = "\
fn f() {
    // panic!(\"in a comment\")
    let s = format!(\"in a call\");
    assert!(c, \"first half \\
             second half\");
    unreachable!(\"lone message\");
    assert_eq!(s.find(\"a value\"), Some(\"another\"), \"compared\");
}
";
    let found: Vec<String> =
        message_prose(text, Code::Strip).into_iter().map(|(_, prose)| prose.trim().to_string()).collect();
    assert_eq!(found, vec!["first half second half", "lone message", "compared"]);
    assert_eq!(message_prose(text, Code::Strip)[1].0, 6, "the message is reported at its own line");
}

/// A guard on the lint itself: the source walk finds the crate and its
/// integration tests, and reads more than a handful of files.
#[test]
fn the_lint_walks_the_whole_crate() {
    let all = source_files();
    assert!(all.len() > 50, "the source walk found only {} files", all.len());
    for root in ["src/lib.rs", "tests/protocol.rs"] {
        assert!(all.iter().any(|(rel, _)| rel == root), "the source walk did not find {root}");
    }
    assert!(
        production_sources().len() < all.len(),
        "no file was recognized as a test module",
    );
}
