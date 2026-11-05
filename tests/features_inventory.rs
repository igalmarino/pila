//! `tests/FEATURES.md` stays true: every test it names exists, and every
//! integration test is named in it.

use std::{collections::BTreeSet, path::Path};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// The `#[test]` functions of a file.
fn tests_in(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut names = BTreeSet::new();
    let mut after_attribute = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "#[test]" {
            after_attribute = true;
            continue;
        }
        if after_attribute {
            if let Some(rest) = line.strip_prefix("fn ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                names.insert(name);
                after_attribute = false;
            } else if !line.starts_with("#[") {
                after_attribute = false;
            }
        }
    }
    names
}

/// The references in backticks: `file::test` and `src/file.rs::test`.
fn references(inventory: &str) -> Vec<(String, String)> {
    inventory
        .split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|r| {
            let (file, test) = r.rsplit_once("::")?;
            let ok = |s: &str| {
                !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "_/.".contains(c))
            };
            (ok(file) && ok(test)).then(|| (file.to_string(), test.to_string()))
        })
        .collect()
}

#[test]
fn features_inventory_is_complete() {
    let inventory = std::fs::read_to_string(Path::new(ROOT).join("tests/FEATURES.md")).unwrap();
    let refs = references(&inventory);
    assert!(refs.len() > 100, "too few references: {}", refs.len());
    let mut missing = Vec::new();
    for (file, test) in &refs {
        let path = if file.starts_with("src/") {
            Path::new(ROOT).join(file)
        } else {
            Path::new(ROOT).join("tests").join(format!("{file}.rs"))
        };
        if !tests_in(&path).contains(test) {
            missing.push(format!("{file}::{test}"));
        }
    }
    assert!(
        missing.is_empty(),
        "FEATURES.md names tests that don't exist: {missing:#?}"
    );

    let named: BTreeSet<(String, String)> = refs.into_iter().collect();
    let mut unlisted = Vec::new();
    for entry in std::fs::read_dir(Path::new(ROOT).join("tests")).unwrap() {
        let path = entry.unwrap().path();
        let Some(file) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().is_none_or(|e| e != "rs") || file == "features_inventory" {
            continue;
        }
        for test in tests_in(&path) {
            if !named.contains(&(file.to_string(), test.clone())) {
                unlisted.push(format!("{file}::{test}"));
            }
        }
    }
    assert!(
        unlisted.is_empty(),
        "tests missing from FEATURES.md: {unlisted:#?}"
    );
}
