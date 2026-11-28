//! A configuration of Pila's own for the standard tools its defaults start
//! (waybar, mako, the launcher fuzzel, swaylock from the swayidle line, and
//! swaybg, which gets Pila's wallpaper: `wallpaper.rs`),
//! so that a session looks like one desktop: the user's `[colors]` (the
//! templates in `resources/tools/` name them `@key@`, `fill`), the title
//! bars' sans-serif at their size (`text::FONT_SIZE`), the windows' 8 px
//! corners on what floats over them (the bar itself stays square). waybar
//! shows the apps' own icons, and symbols for the volume, battery and clock
//! where Font Awesome is installed (text where it isn't: a missing glyph
//! would show as a box). Written at startup and again, on a thread, when a
//! reload changes `[colors]` (`rewrite_in_background`): each tool takes them
//! at its next start.
//!
//! Nothing is written where the tools look for the user's configuration, so
//! it never reaches them outside Pila (another session's waybar) and nothing
//! is left behind: the files go to `$XDG_RUNTIME_DIR/pila-tools` (`prepare`),
//! and the example's own commands (`config::is_example_command`, as written)
//! name them on their command line (`with_configs`: `waybar -c ... -s ...`,
//! `swaylock -C ...` inside the swayidle line, `swaybg -i ... -m fit -c ...`). Only
//! for a tool with no
//! configuration of the user's (`user_has_config`: any file it reads, a
//! broken link too; swaybg reads none), checked at each start of it, so one
//! copied into place takes over. A command written otherwise is the user's and runs as written.
//! `tool-configs = false` turns it off.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::config::Colors;

/// A tool Pila has a configuration for.
pub struct Tool {
    /// The program, as the example's commands run it.
    program: &'static str,
    /// The arguments that point it at Pila's files, inserted after the
    /// program's name; `{dir}` is `prepare`'s directory, and `@key@` a
    /// `[colors]` key's `rrggbb` (`fill`).
    args: &'static str,
    /// Files the tool reads, relative to the config directory, or to the home
    /// directory when they start with `~/`: any of them there means the user
    /// has a configuration of their own.
    existing: &'static [&'static str],
    /// What Pila writes, relative to `prepare`'s directory.
    files: &'static [(&'static str, &'static str)],
    /// What it writes instead where an icon font is installed
    /// (`icon_font_installed`); empty for a tool with no such variant.
    with_icons: &'static [(&'static str, &'static str)],
}

const WAYBAR_STYLE: (&str, &str) = (
    "waybar/style.css",
    include_str!("../resources/tools/waybar/style.css"),
);

const TOOLS: &[Tool] = &[
    Tool {
        program: "waybar",
        args: "-c {dir}/waybar/config.jsonc -s {dir}/waybar/style.css",
        existing: &["waybar/config", "waybar/config.jsonc"],
        files: &[
            (
                "waybar/config.jsonc",
                include_str!("../resources/tools/waybar/config.jsonc"),
            ),
            WAYBAR_STYLE,
        ],
        with_icons: &[
            (
                "waybar/config.jsonc",
                include_str!("../resources/tools/waybar/config-icons.jsonc"),
            ),
            WAYBAR_STYLE,
        ],
    },
    Tool {
        program: "fuzzel",
        args: "--config={dir}/fuzzel/fuzzel.ini",
        existing: &["fuzzel/fuzzel.ini"],
        files: &[(
            "fuzzel/fuzzel.ini",
            include_str!("../resources/tools/fuzzel/fuzzel.ini"),
        )],
        with_icons: &[],
    },
    Tool {
        program: "mako",
        args: "-c {dir}/mako/config",
        existing: &["mako/config", "~/.mako/config"],
        files: &[(
            "mako/config",
            include_str!("../resources/tools/mako/config"),
        )],
        with_icons: &[],
    },
    Tool {
        // No file of its own: `prepare` draws the wallpaper (`wallpaper.rs`).
        // `fit`, not `fill`: the whole picture on any monitor's shape (its
        // mark is in a corner, which `fill` would crop), the rest in
        // `background`, the picture's own. A color without `#`, which the
        // shell would take for a comment.
        program: "swaybg",
        args: "-i {dir}/swaybg/wallpaper.png -m fit -c @background@",
        existing: &[],
        files: &[],
        with_icons: &[],
    },
    Tool {
        program: "swaylock",
        args: "-C {dir}/swaylock/config",
        existing: &["swaylock/config", "~/.swaylock/config"],
        files: &[(
            "swaylock/config",
            include_str!("../resources/tools/swaylock/config"),
        )],
        with_icons: &[],
    },
];

/// The directory Pila's files go to: `pila-tools` in `runtime`.
pub fn dir_in(runtime: &Path) -> PathBuf {
    runtime.join("pila-tools")
}

/// A premultiplied color's `rrggbb`, its alpha left out (the tools draw
/// these opaque: `snap-preview` is the accent).
fn rgb_hex(color: [f32; 4]) -> String {
    let [r, g, b] = crate::wallpaper::rgb8(color);
    format!("{r:02x}{g:02x}{b:02x}")
}

/// Where swaybg's wallpaper goes, relative to `prepare`'s directory (its
/// `args` name it).
const WALLPAPER: &str = "swaybg/wallpaper.png";

/// Write `contents` to `relative` in `dir`: to a file of its own, renamed
/// into place.
fn write_into(dir: &Path, relative: &str, contents: &[u8]) -> std::io::Result<()> {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = partial_path(&path);
    std::fs::write(&partial, contents)?;
    std::fs::rename(&partial, &path)
}

/// Where `path` is written before it's renamed into place: a file of its
/// own for each write, even two of the same file at once (another Pila of
/// the same user, nested, has another pid).
fn partial_path(path: &Path) -> PathBuf {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.with_extension(format!("partial-{}-{n}", std::process::id()))
}

/// `template` with each `@key@` of `[colors]` replaced by its `rrggbb`.
pub fn fill(template: &str, colors: &Colors) -> String {
    let mut text = template.to_string();
    for (key, color) in colors.named() {
        text = text.replace(&format!("@{key}@"), &rgb_hex(color));
    }
    text
}

/// Write every tool's files into `dir` in `colors`, waybar's with symbols if
/// `icon_font`, and the wallpaper. Each is written to a file of its own and
/// renamed into place, so a tool starting meanwhile, or another Pila of the
/// same user (nested), never reads one half written.
pub fn prepare(dir: &Path, icon_font: bool, colors: &Colors) -> std::io::Result<()> {
    for tool in TOOLS {
        let files = if icon_font && !tool.with_icons.is_empty() {
            tool.with_icons
        } else {
            tool.files
        };
        for (relative, contents) in files {
            write_into(dir, relative, fill(contents, colors).as_bytes())?;
        }
    }
    let wallpaper = crate::wallpaper::png(colors).map_err(std::io::Error::other)?;
    write_into(dir, WALLPAPER, &wallpaper)
}

/// `prepare` in `$XDG_RUNTIME_DIR` (a session's own, gone at logout), with
/// symbols for waybar where Font Awesome is installed; the directory, or
/// `None` (logged) if there's none or it can't be written. At startup, before
/// the event loop runs, or on a thread: it reads the font directories.
pub fn prepare_for_session(colors: &Colors) -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
    let dir = dir_in(Path::new(&runtime));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let icon_font = icon_font_installed(&font_dirs(home.as_deref()));
    match prepare(&dir, icon_font, colors) {
        Ok(()) => {
            info!(dir = %dir.display(), icon_font, "tool configs ready");
            Some(dir)
        }
        Err(err) => {
            warn!(dir = %dir.display(), %err, "can't write the tool configs");
            None
        }
    }
}

/// `prepare_for_session` again in `colors`, on a thread of its own (file I/O
/// off the event loop), after a reload changed `[colors]`.
pub fn rewrite_in_background(colors: Colors) {
    // One rewrite at a time: two reloads in a row (an editor saving twice)
    // write their files one after the other, never mixed.
    static REWRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let spawned = std::thread::Builder::new()
        .name("pila-tool-configs".into())
        .spawn(move || {
            use crate::sync::Locked;
            let _one_at_a_time = REWRITING.locked();
            prepare_for_session(&colors);
        });
    if let Err(err) = spawned {
        warn!(%err, "can't rewrite the tool configs in the new colors");
    }
}

/// Whether the user has a configuration of their own for `tool`: any file it
/// reads under `config_home` (or `home`, for the tools that also look
/// there), a broken link included.
fn has_config(tool: &Tool, config_home: Option<&Path>, home: Option<&Path>) -> bool {
    tool.existing.iter().any(|path| {
        let path = match path.strip_prefix("~/") {
            Some(rest) => match home {
                Some(home) => home.join(rest),
                None => return false,
            },
            None => match config_home {
                Some(config_home) => config_home.join(path),
                None => return false,
            },
        };
        path.symlink_metadata().is_ok()
    })
}

/// `has_config` in the user's directories (`XDG_CONFIG_HOME`, `HOME`).
pub fn user_has_config(tool: &Tool) -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    has_config(
        tool,
        crate::config::config_home().as_deref(),
        home.as_deref(),
    )
}

/// `command` with `args` after each tool it runs that `configured` says has
/// no configuration of the user's, pointing it at `dir`'s files (and naming
/// `colors` where they say so): a tool is
/// its name as a word of its own (between the start or end, blanks, quotes
/// and the shell's `;&|()`), so `'swaylock -f'` inside swayidle's line counts.
/// A `dir` the shell would need quoted (blanks, quotes, `$`) is left out, and
/// the command runs as written.
pub fn with_configs(
    command: &str,
    dir: &Path,
    colors: &Colors,
    configured: impl Fn(&Tool) -> bool,
) -> String {
    let Some(dir) = dir.to_str().filter(|dir| {
        dir.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+@:,=".contains(c))
    }) else {
        warn!(dir = %dir.display(), "the tool configs' directory needs quoting: not used");
        return command.to_string();
    };
    let edge = |c: Option<char>| c.is_none_or(|c| c.is_whitespace() || "'\";&|()".contains(c));
    let mut result = command.to_string();
    for tool in TOOLS {
        if configured(tool) {
            continue;
        }
        let args = fill(&tool.args.replace("{dir}", dir), colors);
        let mut from = 0;
        while let Some(at) = result[from..].find(tool.program).map(|at| at + from) {
            let end = at + tool.program.len();
            if edge(result[..at].chars().next_back()) && edge(result[end..].chars().next()) {
                result.insert_str(end, &format!(" {args}"));
                from = end + args.len() + 1;
            } else {
                from = end;
            }
        }
    }
    result
}

/// Where fontconfig looks for fonts by default: the user's data directory and
/// `~/.fonts`, then each of `XDG_DATA_DIRS`' `fonts` (and NixOS's `X11/fonts`).
fn font_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
    let mut dirs = Vec::new();
    match var("XDG_DATA_HOME") {
        Some(data) => dirs.push(PathBuf::from(data).join("fonts")),
        None => dirs.extend(home.map(|home| home.join(".local/share/fonts"))),
    }
    dirs.extend(home.map(|home| home.join(".fonts")));
    let data_dirs = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in std::env::split_paths(&data_dirs) {
        dirs.push(dir.join("fonts"));
        dirs.push(dir.join("X11/fonts"));
    }
    dirs
}

/// Whether Font Awesome is installed: a font file under `dirs` with
/// "awesome" in its path (Arch's `Font Awesome 7 Free-Solid-900.otf`,
/// Fedora's `fontawesome-6-free-fonts/`, Debian's `font-awesome/`). Its
/// symbols waybar's config uses are at the same code points in Font Awesome
/// 4 (Debian, Ubuntu) and 5 to 7.
pub(crate) fn icon_font_installed(dirs: &[PathBuf]) -> bool {
    fn walk(dir: &Path, depth: u32) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            // Followed: font directories are often links (NixOS).
            if path.is_dir() {
                return depth > 0 && walk(&path, depth - 1);
            }
            let name = path.to_string_lossy().to_lowercase();
            name.contains("awesome") && (name.ends_with(".otf") || name.ends_with(".ttf"))
        })
    }
    dirs.iter().any(|dir| walk(dir, 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[colors]`' defaults as `config.example.toml` writes them: key and
    /// `rrggbb`.
    fn example_hex() -> Vec<(String, String)> {
        let example: toml::Table = toml::from_str(crate::config::EXAMPLE).unwrap();
        example["colors"]
            .as_table()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap()[1..7].to_lowercase()))
            .collect()
    }

    fn colors(toml: &str) -> Colors {
        crate::config::Config::from_toml(toml, &|_| None)
            .unwrap()
            .colors
    }

    fn templates() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
        TOOLS
            .iter()
            .flat_map(|t| t.files.iter().chain(t.with_icons))
    }

    /// Every `@key@` in `text` (lowercase: `@DEFAULT_AUDIO_SINK@` is wpctl's).
    fn keys_in(text: &str) -> Vec<&str> {
        text.split('@')
            .skip(1)
            .step_by(2)
            .filter(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
            .collect()
    }

    /// Every `#rrggbb` (CSS, mako) or `rrggbb[aa]` value (fuzzel, swaylock)
    /// in `text`.
    fn colors_in(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            for word in line.split(|c: char| !c.is_ascii_alphanumeric()) {
                let hex = word.len() == 6 || word.len() == 8;
                if hex && word.chars().all(|c| c.is_ascii_hexdigit()) {
                    found.push(word[..6].to_lowercase());
                }
            }
        }
        found
    }

    /// The tools' colors are `[colors]`' (the user's, `fill`), not written
    /// into the templates: a user's `[colors]` changes the tools too.
    #[test]
    fn every_color_is_a_colors_key() {
        let keys: Vec<String> = example_hex().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys.len(), colors("").named().len());
        for (path, text) in templates() {
            let used = keys_in(text);
            assert!(
                !used.is_empty() || path.ends_with(".jsonc"),
                "{path} has no colors"
            );
            for key in used {
                assert!(keys.iter().any(|k| k == key), "{path}: @{key}@");
            }
            for color in colors_in(text) {
                // Transparent, for what swaylock must not draw.
                assert_eq!(color, "000000", "{path}: a color not from [colors]");
            }
        }
    }

    /// Filled in the defaults, the templates have the example's colors
    /// (premultiplied `snap-preview` back to its `rrggbb` too).
    #[test]
    fn the_defaults_fill_in_as_the_example_writes_them() {
        let defaults = colors("");
        for (key, hex) in example_hex() {
            assert_eq!(fill(&format!("@{key}@"), &defaults), hex, "{key}");
        }
        for (path, text) in templates() {
            let filled = fill(text, &defaults);
            assert!(keys_in(&filled).is_empty(), "{path}: {filled}");
        }
    }

    #[test]
    fn the_users_colors_fill_the_templates() {
        let mine = colors("[colors]\nbar-inactive = \"#123456\"\nsnap-preview = \"#ff000080\"\n");
        let fuzzel = fill(include_str!("../resources/tools/fuzzel/fuzzel.ini"), &mine);
        assert!(fuzzel.contains("\nbackground=123456ff\n"), "{fuzzel}");
        assert!(fuzzel.contains("\nselection=ff0000ff\n"), "{fuzzel}");
        let waybar = fill(WAYBAR_STYLE.1, &mine);
        assert!(waybar.contains("background: #123456;"), "{waybar}");
    }

    /// The tools' text is the title bars' size (`text::FONT_SIZE`), in
    /// pixels: fuzzel's and mako's points would be a third bigger.
    #[test]
    fn the_tools_text_is_the_title_bars_size() {
        let px = crate::text::FONT_SIZE as u32;
        for (text, size) in [
            (WAYBAR_STYLE.1, format!("font-size: {px}px;")),
            (
                include_str!("../resources/tools/fuzzel/fuzzel.ini"),
                format!("font=sans-serif:pixelsize={px}\n"),
            ),
            (
                include_str!("../resources/tools/mako/config"),
                format!("font=sans-serif {px}px\n"),
            ),
        ] {
            assert!(text.contains(&size), "no {size:?} in {text}");
        }
    }

    /// fuzzel sizes its text by each monitor's DPI where every monitor's
    /// scale is 1 (`dpi-aware=auto`, its default): bigger on a laptop's
    /// dense panel than on a desktop monitor beside it. Pila's own drawing,
    /// waybar's and mako's follow the scale alone, so fuzzel's must too.
    #[test]
    fn fuzzel_is_sized_by_the_scale_not_the_dpi() {
        let ini = include_str!("../resources/tools/fuzzel/fuzzel.ini");
        let (_, main) = ini.split_once("[main]\n").unwrap();
        let main = main.split("\n[").next().unwrap();
        assert!(
            main.lines().any(|l| l.trim() == "dpi-aware=no"),
            "no dpi-aware=no in [main]"
        );
    }

    #[test]
    fn waybars_configs_are_json_and_alike() {
        let text: serde_json::Value =
            serde_json::from_str(include_str!("../resources/tools/waybar/config.jsonc")).unwrap();
        let icons: serde_json::Value =
            serde_json::from_str(include_str!("../resources/tools/waybar/config-icons.jsonc"))
                .unwrap();
        for config in [&text, &icons] {
            // Pila's workspaces, through the standard protocol, and the apps'
            // own icons in the window list.
            assert_eq!(config["modules-left"][0], "ext/workspaces");
            assert_eq!(config["wlr/taskbar"]["format"], "{icon} {title}");
            assert_eq!(config["modules-right"], text["modules-right"]);
        }
        // Text alone without the font; with it, symbols only from the code
        // points Font Awesome 4 shares with 5 to 7.
        let shared = ['\u{f017}', '\u{f026}', '\u{f027}', '\u{f028}', '\u{f1e6}']
            .into_iter()
            .chain('\u{f240}'..='\u{f244}');
        let shared: Vec<char> = shared.collect();
        let private = |c: &char| ('\u{e000}'..='\u{f8ff}').contains(c);
        let raw = |v: &serde_json::Value| v.to_string();
        assert!(!raw(&text).chars().any(|c| private(&c)));
        let used: Vec<char> = raw(&icons).chars().filter(private).collect();
        assert!(!used.is_empty());
        for c in used {
            assert!(
                shared.contains(&c),
                "U+{:04X} isn't in Font Awesome 4",
                c as u32
            );
        }
    }

    #[test]
    fn the_icon_font_is_found_by_its_files_name() {
        let dir = std::env::temp_dir().join(format!("pila-icon-font-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fonts = dir.join("fonts");
        std::fs::create_dir_all(fonts.join("TTF")).unwrap();
        std::fs::write(fonts.join("TTF/DejaVuSans.ttf"), "").unwrap();
        assert!(!icon_font_installed(std::slice::from_ref(&fonts)));
        for path in [
            "OTF/Font Awesome 7 Free-Solid-900.otf",
            "fontawesome-6-free-fonts/Font Awesome 6 Free-Solid-900.otf",
            "truetype/font-awesome/fontawesome-webfont.ttf",
        ] {
            let _ = std::fs::remove_dir_all(&fonts);
            let file = fonts.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "").unwrap();
            assert!(icon_font_installed(std::slice::from_ref(&fonts)), "{path}");
        }
        // Not a font: its licence.
        let _ = std::fs::remove_dir_all(&fonts);
        std::fs::create_dir_all(fonts.join("font-awesome")).unwrap();
        std::fs::write(fonts.join("font-awesome/LICENSE.txt"), "").unwrap();
        assert!(!icon_font_installed(std::slice::from_ref(&fonts)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_link_counts_as_the_users() {
        let dir = std::env::temp_dir().join(format!("pila-tool-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fuzzel")).unwrap();
        let fuzzel = TOOLS.iter().find(|t| t.program == "fuzzel").unwrap();
        assert!(!has_config(fuzzel, Some(&dir), None));
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("fuzzel/fuzzel.ini")).unwrap();
        assert!(has_config(fuzzel, Some(&dir), None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tools_are_found_as_words_of_their_own() {
        let dir = Path::new("/run/user/1000/pila-tools");
        let none = |_: &Tool| false;
        assert_eq!(
            with_configs("mako", dir, &colors(""), none),
            "mako -c /run/user/1000/pila-tools/mako/config"
        );
        assert_eq!(
            with_configs("swayidle timeout 5 'swaylock -f'", dir, &colors(""), none),
            "swayidle timeout 5 'swaylock -C /run/user/1000/pila-tools/swaylock/config -f'"
        );
        // Not part of another word, not a tool the user configured.
        assert_eq!(
            with_configs("makoctl reload", dir, &colors(""), none),
            "makoctl reload"
        );
        assert_eq!(
            with_configs("my-waybar", dir, &colors(""), none),
            "my-waybar"
        );
        assert_eq!(with_configs("waybar", dir, &colors(""), |_| true), "waybar");
        // swaybg's arguments name `[colors] background`, as the shell takes it.
        let dark = colors("[colors]\nbackground = \"#101112\"\n");
        assert_eq!(
            with_configs("swaybg", dir, &dark, none),
            format!(
                "swaybg -i {}/swaybg/wallpaper.png -m fit -c 101112",
                dir.display()
            )
        );
        // A directory the shell would split: the command as written.
        let spaced = Path::new("/run/my user/pila-tools");
        assert_eq!(with_configs("waybar", spaced, &colors(""), none), "waybar");
    }

    /// What floats over the windows (mako's notifications, fuzzel's window,
    /// waybar's tooltips) has the windows' corners, the example's
    /// `corner-radius`; the bar, along the screen's edge, and fuzzel's
    /// selection stay square.
    #[test]
    fn floating_tools_have_the_windows_corners() {
        let radius = crate::config::Config::from_toml("", &|_| None)
            .unwrap()
            .corner_radius;
        let file = |path: &str| {
            TOOLS
                .iter()
                .flat_map(|tool| tool.files)
                .find(|(p, _)| *p == path)
                .map(|(_, text)| *text)
                .unwrap()
        };
        assert!(file("mako/config").contains(&format!("\nborder-radius={radius}\n")));
        let fuzzel = file("fuzzel/fuzzel.ini");
        assert!(fuzzel.contains(&format!("\nradius={radius}\n")));
        assert!(fuzzel.contains("\nselection-radius=0\n"));
        let style = file("waybar/style.css");
        let tooltip = &style[style.find("tooltip {").unwrap()..];
        let tooltip = &tooltip[..tooltip.find('}').unwrap()];
        assert!(
            tooltip.contains(&format!("border-radius: {radius}px;")),
            "{tooltip}"
        );
        assert!(style.starts_with("/*") && style.contains("border-radius: 0;"));
    }

    /// Two writes of the same file at once (two reloads that change
    /// `[colors]`, each rewriting on a thread of its own) never write the
    /// same temporary file: one would rename the other's half written.
    #[test]
    fn each_write_has_a_temporary_file_of_its_own() {
        let path = Path::new("/run/user/1000/pila-tools/fuzzel/fuzzel.ini");
        assert_ne!(partial_path(path), partial_path(path));
        assert_eq!(partial_path(path).parent(), path.parent());
    }

    #[test]
    fn every_file_a_tools_args_name_is_written() {
        let dir = std::env::temp_dir().join(format!("pila-tool-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for icon_font in [false, true] {
            prepare(&dir, icon_font, &colors("")).unwrap();
            for tool in TOOLS {
                let args = tool.args.replace("{dir}", &dir.display().to_string());
                for word in args.split([' ', '=']).filter(|w| w.starts_with('/')) {
                    assert!(Path::new(word).is_file(), "{word}");
                }
            }
        }
        // Nothing half written left behind.
        let mut partial = Vec::new();
        for tool in ["waybar", "fuzzel", "mako", "swaylock", "swaybg"] {
            for entry in std::fs::read_dir(dir.join(tool)).unwrap() {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                if name.contains("partial") {
                    partial.push(name);
                }
            }
        }
        assert!(partial.is_empty(), "{partial:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
