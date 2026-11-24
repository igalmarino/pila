# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# Pila: a stacking Wayland compositor

A daily-driver **stacking** Wayland compositor (crate/binary name `pila`) written in **Rust**
on **Smithay**'s main branch (a git dependency pinned to a `rev` in `Cargo.toml`, as
cosmic-comp does: 0.7.0, the last release, is from June 2025; for now from Pila's fork,
`igalmarino/smithay`, branch `pila`: a rev of Smithay's plus the fixes `Cargo.toml` lists).

Two more files go with this one: `tests/CLAUDE.md` (how the integration tests work) and
`scripts/CLAUDE.md` (`scripts/check`, `scripts/apps`, the demo video); read the one for the
directory before working in `tests/` or `scripts/`. Releases and packaging are in the
`release` skill (`.claude/skills/release`). `AGENTS.md` inlines this file for other agents
(Antigravity), which cut it past 24,000 bytes: `docs_names` checks it fits.

## Environment

- You run inside a dev container (Arch Linux base). The host is Arch Linux; the user logs
  into Pila itself through a greetd greeter (`scripts/install`), and it's the only desktop
  there: a build that doesn't start leaves no other session to fall back on, so try it with
  `scripts/tty` first.
- The compositor is developed **nested**: Smithay's winit backend opens it as a window on
  the host desktop. `WAYLAND_DISPLAY` points at the host socket for that purpose
  (`/run/host-wayland/wayland-0`, which `scripts/link-wayland` makes for whichever session
  the container was entered from: on Pila a wp-security-context socket, so privileged
  protocols are hidden here too; on GNOME a hard link to the session's socket); the compositor
  creates its own socket in `$XDG_RUNTIME_DIR` and clients launched from it connect there.
  The nested window's app id is `pila`: the user's host config opens it on eDP-1, unfocused
  (`open-on-output`, `open-focused = false`), away from the work on DP-1.
- The user drives the container from the host with `scripts/dev` (`up`, `shell`, `claude`,
  `cargo ...`, `rebuild` after a Dockerfile change; see CONTRIBUTING.md); any `scripts/dev`
  command run from a session makes its Wayland socket again. Logging out and back in leaves
  `/run/host-wayland/` empty until then, and nested runs fail with `EventLoopCreation`. The
  sandboxed socket is `scripts/sandbox-socket` (a wp-security-context client, standard-library
  Python), which `tests/surface_protocols.rs` also uses.
- The DRM/KMS + libinput backend (`--backend udev`, `udev/`) cannot be tested in this
  container (no seat, no TTY); there it exits with "could not start a session". It needs
  the host, from a TTY. Write that code, but ask the user to test it.
- You can't see the screen. Ask the user to confirm anything visual.
- No root and no package manager access. If a system library or tool is missing, say
  which Arch package is needed so the user can add it to `.devcontainer/Dockerfile`.

## Commands

```sh
cargo build
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                      # cargo test <name_substring> for one test
timeout 10 cargo run            # nested smoke test; never leave it running
```

- Pila ships no desktop programs of its own (no bar, launcher, on-screen display or
  screenshot tool): its defaults are standard tools packaged by Arch, Debian and Fedora
  (fuzzel, waybar, wpctl, brightnessctl, playerctl, grim, slurp, wl-copy), named in
  `config.example.toml` alone (swayidle and swaylock to lock and turn the monitors off,
  swaybg for the wallpaper, which Pila draws as a PNG in `[colors]`: `wallpaper.rs`). Where
  a packaged tool does a job well, Pila uses it rather than growing the feature; what only a
  compositor can do, Pila draws: title bars, the window switcher's live previews and the overview
  (`switcher_view.rs`).
  Programs an older install left are in `scripts/install-paths`' `removed_files`.

- CI (`.github/workflows/ci.yml`, every push to `main` and every PR) runs `cargo fmt --check`,
  then clippy, a release build and the tests with `--locked` (a `Cargo.lock` out of date
  with `Cargo.toml` fails there, not locally), does a staged install and runs
  `shellcheck --severity=warning --external-sources` on the scripts (and parses the Python
  ones): run that after editing one. The other workflows are in `.github/workflows/` (the
  package ones in the `release` skill); `aur.yml` publishes only once approved: a release's
  `pkgver` commit by Claude, any other change by the user.
- Every feature gets integration tests (`tests/*.rs`; how they work is in `tests/CLAUDE.md`)
  and a new test goes into `tests/FEATURES.md`; only bare-metal paths (`udev/`), real frame
  times (`scripts/check`) and looks stay with the user. `scripts/coverage [--html]` reports line
  coverage per file, a guide to what the tests miss.
- Fixing a bug: first write a test that reproduces it and see it fail, then fix it; the test
  stays in the suite to verify the fix and keep the bug from coming back. If a bug can't be
  tested automatically (bare metal, looks), say so and why.
- Run nested: `cargo run` opens a window on the host desktop and blocks. Don't leave it
  running in the foreground; use `timeout 10 cargo run` for smoke tests or ask the user.
  `cargo run -- -c '<cmd>'` runs `<cmd>` at startup instead of `autostart` (default: the
  `autostart` list in `config.example.toml`; no terminal starts by itself).
- Config: `config.example.toml` (copy to `~/.config/pila/config.toml`, or `--config`);
  `pila --check-config [--config FILE]` validates a file as Pila reads it, plus the
  keymap (`config::check_keymap`), and exits 0/1 (`main::run_config_check`).
  Env overrides: `USAGE` in `main.rs`. Alt is the default because GNOME grabs Super while
  nested.
- Test clients available inside the nested compositor: `foot`, `weston-terminal`,
  `weston-simple-egl`, `wayland-info`, and `gtk4-widget-factory` / `gtk4-demo` (menus,
  popovers, dropdowns: the way to exercise popups and popup grabs). `foot` and GTK use
  cursor-shape (named cursors); `weston-terminal` draws its own cursor surfaces.
  Desktop tools for layer-shell and friends: `swaybg` (Background), `waybar` (Top, exclusive
  zone), `fuzzel` (Overlay, exclusive keyboard; the default launcher), `mako` +
  `notify-send` (notifications), `swaylock` (session lock), `wl-copy`/`wl-paste`
  (ext-data-control), `grim` + `slurp` (screenshots: ext-image-copy-capture), `wtype`
  (virtual keyboard), `wlr-randr` (output management), `wlopm` (output power), `wlsunset`
  (gamma); `kitty` (the default terminal);
  `fcitx5` with Rime's pinyin (input-method-v2; GTK 4, foot and kitty reach it through
  text-input-v3). X11 (via Xwayland): `xterm`, `xeyes`, `xdpyinfo`.
- Bare-metal smoke and performance run: `scripts/check --nested` (release build, log in
  `pila-check.log`) is for you: run it before asking the user to check anything; the user
  runs plain `scripts/check` from a TTY (log in `pila-tty.log`) for bare metal. What it
  checks and when a feature adds a scenario: `scripts/CLAUDE.md`.
- Real apps: `scripts/apps [gtk4|foot|kitty|weston-terminal|xterm|switcher|overview ...]` drags, clicks,
  maximizes and snaps real apps' windows with real input on a sealed headless Pila and checks
  they follow the pointer and leave no grab; `fcitx5-foot` and `fcitx5-gtk4` type Chinese
  through fcitx5 into a terminal and a GTK text field (`scripts/CLAUDE.md`): after changing
  grabs, decorations, fill states, input or input methods, and before a release.
- Demo video: `scripts/demo-video` records captioned scenes on a sealed headless Pila into
  `pila-demo.mp4`, `pila-demo-short.mp4` (not committed) and the README's animated
  `docs/demo.webp` (`--shots` to check the scenes quickly; `scripts/CLAUDE.md`); show them to
  the user before they're committed or published.
- Debugging: `WAYLAND_DEBUG=1` on a client for protocol traces, `RUST_LOG=debug` for the
  compositor, `RUST_LOG=pila::frames=debug` for frame times and missed refreshes (see
  "Performance pillar"). To watch an animation frame by frame: `[animations] speed = 4`
  and `grim` in a loop from a script run with `cargo run -- -c`.
- The login session (the user, on the host) is installed by `scripts/install`
  (`scripts/CLAUDE.md`). Its log is in the journal (`journalctl -t pila -b`) and in
  `~/.local/state/pila/pila.log`, mounted here read-only as `/run/host-pila/pila.log`
  (previous run: `pila.log.old`).
  Taking a change into the session means logging out and back in, which ends this
  Claude session; the user does it.
- Packages (Arch, Debian/Fedora, Nix, and each one's `pila-desktop` metapackage of the
  default tools, whose lists change together): a file added to `scripts/install`'s
  `install_files`, a newly linked library or a changed default tool also goes in them, so
  read the `release` skill first (it covers packaging as well as releases).
- TTY runs (the user, on the host): `scripts/tty [config]` runs the release build
  (`--debug` for the debug one) by hand on a spare VT, under `dbus-run-session`, while the
  login session keeps running; read `pila-tty.log` and the previous run's
  `pila-tty.log.old` in the repo root afterwards. The user's own config lives outside the
  repo (`~/.config/pila`); keep `config.example.toml` valid (a test parses it).
  `scripts/layer-namespaces [config]` runs the same way and lists the namespaces of the layer
  surfaces the user's tools open, for writing `[[layer-rule]]` sections.
- This container is connected to the user's Pila session, so `cargo run` opens a nested
  window inside it. Changes that need a Pila restart are tested by the user.
- Git: small commits straight to `main`, after the user has checked anything visual or
  hardware-related; push to GitHub (`origin`, public, GPL-3.0-or-later) only when asked.

## Language

Everything in the repository is in English: code, identifiers, comments, docs, log
messages, test names, config examples and commit messages, even when the conversation
isn't.

**Naming:** the project is "Pila" in prose. Write `pila` (in code style) for the command,
crate, packages, paths, environment variables and anything users type.

## Distro policy

Pila must not assume a distro, init system or display manager. This machine is Arch with
systemd and a greetd greeter, but the code and the host-side scripts (`scripts/`,
`resources/session/`) must also work elsewhere (Debian, Fedora, Void, Alpine, ...),
including without systemd (elogind or seatd, no per-user D-Bus bus, no journal): check for
optional tools (`systemd-cat`, `logger`, `dbus-run-session`) and degrade, use
`/usr/bin/env bash`, and keep install paths configurable. Name packages generically in
messages ("package dbus"), not with one distro's package manager. `rust-version` in
`Cargo.toml` is the oldest Rust that builds Pila; raise it when a dependency or the code
needs a newer one. The dev container itself is Arch, and its packages are named as Arch's.

## Performance pillar

Pila never makes an output miss a refresh while there is something to show (an
animation, a drag, a client's new frame), at the output's own refresh rate (60, 144 Hz,
...); 60 Hz is the floor it must hold everywhere. It renders on demand: smooth doesn't
mean redrawing when nothing changed. How fast clients draw is up to them.

- **Budget:** the CPU time Pila spends on a frame (scene, elements, submitting the
  render) stays well inside the period. Target: release build on the user's laptop (AMD
  Renoir), typical scenes, **at most 4 ms per output** (a quarter of 60 Hz, still room at
  144 Hz).
- **Measure it:** any change that touches rendering, input or the event loop is checked
  with `RUST_LOG=pila::frames=debug` (`frame_stats.rs`) and `scripts/check --nested` (release
  build) before it's committed; bare-metal numbers come from the user with `scripts/check`
  (see Commands). What each logged number means: the `frame-stats` skill.
- **Rules:**
  - Nothing blocks the event loop: I/O, D-Bus and font loading go to threads or run in the
    background, like `text.rs` and `export_environment`.
  - Work is proportional to damage; expensive paths (offscreen textures, blur, captures) run
    only while they're needed.
  - Frames are paced by vblank (udev) or the host's frame callbacks (nested), never by
    timers. These timers don't pace frames, so they're allowed:
    - the headless test backend's, which has no vblank;
    - the once-a-second wakes for hidden surfaces (`schedule_hidden_frames`,
      `commit_pacing.rs`);
    - the wake for a timestamp a client asked for;
    - the frame callbacks a captured hidden window gets at its capture client's rate;
    - on bare metal, the wait of an output that just redrew with nothing new, until half a
      period before its next vblank (`udev/draw.rs::idle_wait`). A client committing no new
      picture (no buffer, or one without damage) in a loop of frame callbacks or fifo
      barriers would otherwise make it redraw as fast as it loops. A change of Pila's own
      never waits, and a client's new picture waits once (`redraw::Content`, `idle_streak`);
    - on bare metal, the wait of a frame held to be drawn its margin before the vblank
      (`udev/render_late.rs`: the vblank paces it, the timer only places it in the period).
  - Redraws are per output: a change redraws only the outputs it shows on (`redraw.rs`).
    Idle callbacks inserted outside an event source come with a `Pila::wake` ping (see
    "Architecture").
  - One-time costs are paid at startup, not in the first frame that needs them:
    `render::warm_up` when a renderer is created (shaders, an offscreen texture, a draw of
    each kind a window needs); `text::init` shapes and rasterizes common characters on its
    thread before publishing the fonts; a workspace slide's output-sized textures are made
    ahead and kept (`workspace_slide.rs`).

## Protocol policy

Pila prefers **standard protocols**: Wayland core and the official `wayland-protocols`
(stable, staging, and unstable where nothing newer exists). Where both exist, prefer the
`ext-`/staging protocol over an older unstable or vendor one (e.g. `ext-data-control`, not
`zwlr_data_control`). A standard protocol that Smithay doesn't implement (e.g.
`ext-workspace`) is implemented in Pila on the bindings from the
`wayland-protocols` crate, not swapped for a vendor one. (One temporary exception:
xdg-session-management's XML is in that crate's release but no module yet, so
`session_protocol.rs` generates its bindings with `wayland-scanner`; delete it, its
client copy in `tests/common/` and the XML when a release has them.)

A **wlroots protocol** is accepted when there's no standard equivalent yet and a desktop
tool the user runs needs it; each is replaced as soon as an `ext-` version lands in
`wayland-protocols` and Smithay. The accepted ones (all needed by panels and
shells such as waybar): `wlr-layer-shell`, `wlr-screencopy`, `wlr-foreign-toplevel-management`,
`wlr-gamma-control`, `wlr-output-management`, `wlr-output-power-management` and
`virtual-keyboard-unstable-v1`; and `input-method-unstable-v2`, which input methods (fcitx5)
need, until its successor (`xx-input-method`, experimental) lands. Other vendor protocols (KDE,
GNOME, Hyprland, …) are not added, even if Smithay implements them. Features with no protocol
at all go through Pila's own config/IPC.

Privileged protocols are hidden from sandboxed apps: a client that connected through
wp-security-context (Flatpak) fails `state::is_privileged` (`ClientState::security_context`;
Xwayland counts as privileged). Smithay globals take it as their filter (those Smithay
gives none, keyboard-shortcuts-inhibit and ext-idle-notify, are made again hidden with
`state::hide_from_sandboxes`), Pila's own as `GlobalDispatch::can_view` (grep those for
the list; xwayland-keyboard-grab is shown to Xwayland alone, by Smithay). A new
global that exposes or controls other clients, input or outputs must get it too.

## Architecture

The crate is a library (`src/lib.rs`: every module and `Pila`) plus the `pila` command
(`src/main.rs`: arguments, `USAGE`, `--check-config`, the panic hook); the fuzz targets
(`fuzz/`, a nightly workspace of its own) call the library, so code they reach stays in it.

One state struct, `Pila` (`src/state.rs`), implements every Smithay handler trait and is
the calloop event loop's data. Core window management lives in `state.rs`; each feature
beyond it has its own module, and `render.rs`, `frame.rs` and `post_render` are shared by
every backend.

**[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) describes every flow and invariant, a
section per subsystem. Read a subsystem's section before changing it** (each is a bullet
starting with its bold name and module: `grep -n '^- \*\*' docs/ARCHITECTURE.md` lists them). A change to a flow or invariant updates its section
in the same commit, and a new feature adds one. A rename updates the docs and comments
that name the old path too: `docs_names::the_docs_name_only_code_that_exists` fails on a
file or `module::item` that's gone (only when the name is gone from the whole code: a field
removed whose name lives on elsewhere passes, so grep).

What breaks silently anywhere in the code, whichever subsystem you're in:

- **Redraws are on demand, per output** (`redraw.rs`): nothing redraws an output unless
  asked. A change asks for the outputs it shows on (`queue_redraw_output`,
  `queue_redraw_window`, `queue_redraw_rect`; commits and input already do), `queue_redraw`
  only for what's global; a change to the `Space`'s windows goes through `map_window`/
  `unmap_window`/`raise_window`. The tests' headless check (`PILA_HEADLESS_CHECK_REDRAWS`)
  panics on an output that changed with no redraw asked. An idle callback inserted outside
  an event source must ping `Pila::wake`, or it runs after the next event, however late;
  redraws requested from `after_frames` go through `defer_redraws`.
- **Session lock:** the focus types (`focus.rs`) refuse input to anything but a lock
  surface while locked, and `Pila::under` hits nothing else; a new path that tries panics in
  debug builds ("refused while locked"). A new path still refuses what it shouldn't do while
  locked (`is_locked()`) on its own; a new kind of action goes into `Action::examples`
  (`config.rs`), which the lock test runs, and its name into `NAMED_ACTIONS` (or
  `ACTIONS_WITH_ARGUMENT`), which parsing and its error message read and a test checks
  `config.example.toml` lists.
- **Input:** a new input path goes through `process_input_event` (or sets
  `input_activity`) to count as activity, and hit testing through `Pila::under`, which
  must agree with the order `Pila::scene` renders.
- **X11:** `window.toplevel()` may be `None`: use `wl_surface()`, `fill_state`,
  `close_window` and `focus_target`, and branch on `x11_surface()`. Moving a managed X11
  window must tell X (`x11_window_moved`); compare focus through `Pila::keyboard_focus()`.
- **Off the `Space`:** it holds only the active workspace's windows and those on all
  workspaces. Parked and minimized
  ones are `off_space_windows()`, and unmapped ones are elsewhere too: look windows up with
  `window_for_surface`/`window_for_x11`, not by searching the `Space`.
- **Pending vs committed:** layout uses the pending decoration mode (`layout_offset`),
  drawing and hit testing the committed one (`has_title_bar`); `fill_state` reads the
  pending xdg state.
- **Stacking:** a new way to raise a window must end in `Pila::keep_on_top`
  (`stacking.rs`), or always-on-top windows end up below it.
- **Grabs:** `Pila::set_cursor` must be called after `set_grab` (`Focus::Clear` resets the
  cursor). A keyboard grab isn't always a menu's: an input method's holds the keyboard while
  it's active; test for a menu's with `Pila::keyboard_popup_grab`, never `is_grabbed()`.
- **Backends:** code outside them uses `Backend`'s methods rather than matching on it.
- **Panics end the session:** past startup no `unwrap`/`expect` but on a local invariant the
  message states; `.locked()` (`sync.rs`) for a mutex, `get_or_insert` for user data, else
  `let`-`else`, logging what means something went wrong.
- **Settings:** a new one is read when used or re-applied in `apply_config`. Its default is
  written in `config.example.toml` alone, as an active line (examples stay commented out, and
  come before it: `the_examples_come_before_the_value_they_would_replace`): the
  code takes what a user's file leaves out from it (`Config::parse_with`) and has no default
  values of its own (`need` fails without one; `every_default_comes_from_the_example` checks).
  The same goes for the default bindings and gestures, its `[bindings]` and `[gestures]`. Keep
  in sync: `USAGE` in `main.rs` for flags and env vars, and the user docs: `docs/MANUAL.md` (by task)
  and the man page `docs/pila.1` (tests check that both list every default binding, and the
  man page every flag and variable of `USAGE`). A feature users see gets its place in the
  manual.
  Config keys, actions and `pila msg` fields are only added to: a rename keeps the old name
  (serde `alias`), and `tests/fixtures/every-key.toml` and `tests/compat.rs`' field lists
  only grow (CONTRIBUTING.md, "Compatibility").

## Reference code

Smithay's `smallvil` (minimal stacking compositor) and `anvil` (full-featured) examples are
the canonical references. Clone `https://github.com/Smithay/smithay` outside `/workspace`
(e.g. `/tmp`) at the `rev` of the `smithay` dependency in `Cargo.toml` and read them before
inventing new patterns. Moving `rev` forward is a change of its own: follow the `smithay-bump` skill.
