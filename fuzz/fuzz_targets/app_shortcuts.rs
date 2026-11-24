//! What the portal takes from apps and reads back for them: an
//! `app-shortcuts.json` (`app_shortcuts::parse_store`) and every trigger in
//! it, a shortcut trigger an app prefers (`app_shortcuts::parse_trigger`,
//! then `assignable` and `format_trigger`, as a key confirmed is saved), and
//! an app id named by its desktop entry (`portal::app_name`), which is a file
//! name and never a path.
#![no_main]

use std::{path::PathBuf, sync::OnceLock};

use libfuzzer_sys::fuzz_target;
use pila::app_shortcuts::{assignable, format_trigger, parse_store, parse_trigger};

/// A data directory's `applications` with one entry, `org.example.App`.
fn applications() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = PathBuf::from(format!("/tmp/pila-fuzz-apps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("org.example.App.desktop"),
            "[Desktop Entry]\nName=Example\nExec=example\n",
        )
        .unwrap();
        dir
    })
}

/// A trigger as an app sends it or the store holds it: once read and saved
/// (formatted), it reads back the same.
fn trigger(text: &str) {
    let Some(combo) = parse_trigger(text) else {
        return;
    };
    let _ = assignable(&combo);
    let Some(saved) = format_trigger(&combo) else {
        return;
    };
    let read = parse_trigger(&saved).expect("a saved trigger reads back");
    assert_eq!(
        format_trigger(&read).as_deref(),
        Some(saved.as_str()),
        "{text:?} saved as {saved:?} doesn't save the same again"
    );
}

fuzz_target!(|input: (&str, &str, &str)| {
    let (store, preferred, app_id) = input;
    if let Ok(store) = parse_store(store) {
        for text in store.values().flat_map(|shortcuts| shortcuts.values()) {
            trigger(text);
        }
    }
    trigger(preferred);

    let name = pila::portal::app_name(app_id, std::slice::from_ref(applications()));
    let id = app_id.trim();
    if id.is_empty() {
        assert_eq!(name, None);
    } else if id.starts_with('.') || id.contains(['/', '\0']) {
        assert_eq!(name.as_deref(), Some(id), "a path read as an app's entry");
    }
});
