//! The docs name only code that exists: in `docs/`, the `CLAUDE.md`s,
//! `CONTRIBUTING.md` and the comments in `src/`, every file (`foo.rs`) and
//! every path into Pila's code (`module::item`, `Type::method`,
//! `udev/draw.rs::idle_until`) in backticks. Names of other projects (Smithay's,
//! a protocol's) aren't checked: only paths that start at one of Pila's modules
//! or types.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                files_under(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Identifiers in `text`.
fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
}

/// `text` without its `//` comments (so a comment can't vouch for itself).
fn code_only(text: &str) -> String {
    text.lines()
        .map(|line| line.find("//").map_or(line, |at| &line[..at]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What's between backticks in `text`.
fn quoted(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

#[test]
fn the_docs_name_only_code_that_exists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    for dir in ["src", "tests", "fuzz/fuzz_targets"] {
        files_under(&root.join(dir), &mut sources);
    }
    sources.push(root.join("build.rs"));

    // Every identifier of the code, the file names, Pila's modules and the
    // types it declares.
    let mut names = HashSet::new();
    let mut types = HashSet::new();
    let texts: Vec<(PathBuf, String)> = sources
        .iter()
        .map(|p| (p.clone(), std::fs::read_to_string(p).unwrap()))
        .collect();
    for (_, text) in &texts {
        let code = code_only(text);
        let words: Vec<&str> = identifiers(&code).collect();
        for pair in words.windows(2) {
            if ["struct", "enum", "trait", "type"].contains(&pair[0]) {
                types.insert(pair[1].to_string());
            }
        }
        names.extend(words.into_iter().map(str::to_string));
    }
    let file_names: HashSet<String> = sources
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_string))
        .collect();
    let mut modules: HashSet<String> = sources
        .iter()
        .filter(|p| p.starts_with(root.join("src")))
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_string))
        .filter(|m| m != "mod" && m != "lib" && m != "main")
        .collect();
    modules.extend(["udev", "handlers", "grabs"].map(String::from));

    // The docs, and the comments of the code.
    let mut docs: Vec<(String, String)> = [
        "docs/ARCHITECTURE.md",
        "docs/MULTI_GPU.md",
        "CLAUDE.md",
        "tests/CLAUDE.md",
        "scripts/CLAUDE.md",
        "CONTRIBUTING.md",
    ]
    .iter()
    .map(|p| {
        (
            p.to_string(),
            std::fs::read_to_string(root.join(p)).unwrap(),
        )
    })
    .collect();
    for (path, text) in &texts {
        if !path.starts_with(root.join("src")) {
            continue;
        }
        let comments: Vec<&str> = text
            .lines()
            .filter_map(|line| line.find("//").map(|at| &line[at..]))
            .collect();
        let rel = path.strip_prefix(root).unwrap().display().to_string();
        docs.push((rel, comments.join("\n")));
    }

    let mut stale = Vec::new();
    for (doc, text) in &docs {
        for token in quoted(text) {
            let token = token.trim().trim_end_matches("()");
            if token.contains(char::is_whitespace) || token.contains('*') || token.is_empty() {
                continue;
            }
            let (path, item) = match token.split_once("::") {
                Some((path, item)) => (path, Some(item)),
                None => (token, None),
            };
            let item = item.map(|i| i.rsplit("::").next().unwrap_or(i));
            if let Some(file) = path.strip_suffix(".rs") {
                let name = format!("{}.rs", file.rsplit('/').next().unwrap_or(file));
                if !file_names.contains(&name) {
                    stale.push(format!("{doc}: `{token}` (no such file)"));
                } else if let Some(item) = item.filter(|i| !names.contains(*i)) {
                    stale.push(format!("{doc}: `{token}` (no {item})"));
                }
                continue;
            }
            let Some(item) = item else {
                continue;
            };
            let first = path.split("::").next().unwrap_or(path);
            let ours = modules.contains(first) || types.contains(first) || first == "crate";
            if ours && identifiers(item).count() == 1 && !names.contains(item) {
                stale.push(format!("{doc}: `{token}` (no {item})"));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "names in the docs that aren't in the code:\n{}",
        stale.join("\n")
    );
}
