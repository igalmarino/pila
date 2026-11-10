# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# Pila: a stacking Wayland compositor

A daily-driver **stacking** Wayland compositor (crate/binary name `pila`) written in **Rust**
on **Smithay**'s main branch (a git dependency pinned to a `rev` in `Cargo.toml`, as niri
and cosmic-comp do: 0.7.0, the last release, is from June 2025).

## Environment

- You run inside a dev container (Arch Linux base). The host is Arch Linux; the user logs
  into Pila itself through Noctalia's greeter (`scripts/install`), and it's the only desktop
  there: a build that doesn't start leaves no other session to fall back on, so try it with
  `scripts/tty` first.
- The compositor is developed **nested**: Smithay's winit backend opens it as a window on
  the host desktop. `WAYLAND_DISPLAY` points at the host socket for that purpose
  (`/run/host-wayland/wayland-0`, which `scripts/link-wayland` makes for whichever session
  the container was entered from: on Pila a wp-security-context socket, so privileged
  protocols are hidden here too; on GNOME a hard link to the session's socket); the compositor
  creates its own socket in `$XDG_RUNTIME_DIR` and clients launched from it connect there.
- The user drives the container from the host with `scripts/dev` (`up`, `shell`, `claude`,
  `cargo ...`; see CONTRIBUTING.md); any `scripts/dev` command run from a session makes
  its Wayland socket again. Logging out and back in leaves `/run/host-wayland/` empty until
  then, and nested runs fail with `EventLoopCreation`.
- The DRM/KMS + libinput backend (`--backend udev`, `udev/`) cannot be tested in this
  container (no seat, no TTY); there it exits with "could not start a session". It needs
  the host, from a TTY. Write that code, but ask the user to test it.
- You can't see the screen. Ask the user to confirm anything visual.
- No root and no package manager access. If a system library or tool is missing, say
  which Arch package is needed so the user can add it to `.devcontainer/Dockerfile`.

## Commands

- Lint: `cargo clippy --all-targets -- -D warnings`
- CI (`.github/workflows/ci.yml`, every push) also runs `cargo fmt --check`, builds with
  `--locked`, does a staged install and runs `shellcheck --severity=warning
  --external-sources` on the scripts: run that after editing one. `compat.yml` (`rust-version`,
  Debian, Ubuntu, Fedora, Nix, `cargo audit`) runs when dependencies or packaging change, weekly
  and by hand before a release. `packages.yml` builds the `.deb` and `.rpm` of a release.
  `vkms.yml` (by hand, an experiment) runs the udev backend on the kernel's virtual KMS
  driver in the runner's VM: frames, a gamma change, a monitor plugged in and out.
- Test: `cargo test`; single test: `cargo test <name_substring>`. Every feature gets
  integration tests (`tests/*.rs`; how they work is in `tests/CLAUDE.md`) and a new test
  goes into `tests/FEATURES.md`; only bare-metal paths (`udev/`), real frame times
  (`scripts/check`) and looks stay with the user.
- Fixing a bug: first write a test that reproduces it and see it fail, then fix it; the test
  stays in the suite to verify the fix and keep the bug from coming back. If a bug can't be
  tested automatically (bare metal, looks), say so and why.
- Run nested: `cargo run` opens a window on the host desktop and blocks. Don't leave it
  running in the foreground; use `timeout 10 cargo run` for smoke tests or ask the user.
  `cargo run -- -c '<cmd>'` runs `<cmd>` at startup instead of the terminal.
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
  zone), `fuzzel` (Overlay, exclusive keyboard; `PILA_LAUNCHER=fuzzel`), `mako` +
  `notify-send` (notifications), `swaylock` (session lock), `wl-copy`/`wl-paste`
  (ext-data-control), `grim` + `slurp` (screenshots: ext-image-copy-capture). X11 (via
  Xwayland): `xterm`, `xeyes`, `xdpyinfo`.
- Bare-metal smoke and performance run: `scripts/check --nested` (release build, log in
  `pila-check.log`) is for you: run it before asking the user to check anything; the user
  runs plain `scripts/check` from a TTY (log in `pila-tty.log`) for bare metal. What it
  checks and when a feature adds a scenario: `scripts/CLAUDE.md`.
- Debugging: `WAYLAND_DEBUG=1` on a client for protocol traces, `RUST_LOG=debug` for the
  compositor, `RUST_LOG=pila::frames=debug` for frame times and missed refreshes (see
  "Performance pillar"). To watch an animation frame by frame: `[animations] speed = 4`
  and `grim` in a loop from a script run with `cargo run -- -c`.
- The login session (the user, on the host): `scripts/install` builds the release binary
  (with the host's cargo if there is one, else `--container`; `--no-build`) and installs it
  with `resources/session/` (the `pila-session` wrapper and the `wayland-sessions` entry,
  whose `@BINDIR@` it fills in) and the portal configs; `scripts/uninstall` removes them.
  Paths come from `scripts/install-paths` (`PREFIX`, `BINDIR`, `DATADIR`, `SYSCONFDIR`,
  `DESTDIR`), sourced by both. Its log is in the journal (`journalctl -t pila -b`) and in `~/.local/state/pila/pila.log`,
  mounted here read-only as `/run/host-pila/pila.log` (previous run: `pila.log.old`).
  Taking a change into the session means logging out and back in, which ends this
  Claude session; the user does it.
- Arch package: `packaging/aur/pila-git`. Keep `depends` in its `PKGBUILD` in step with the
  libraries Pila links (`readelf -d`) and README's package table. Packaging and releases:
  the `release` skill.
- Nix package: `flake.nix` and `package.nix` (the same files as `scripts/install`: a file added
  to `install_files` is added there too). No Nix in this container: `compat.yml`'s Nix job builds it and
  starts Pila from it. `flake.lock` pins nixpkgs; a dependency on a new system library goes in
  `buildInputs` there too.
- TTY runs (the user, on the host): `scripts/tty [config]` runs the release build
  (`--debug` for the debug one) by hand on a spare VT, under `dbus-run-session`, while the
  login session keeps running; read `pila-tty.log` and the previous run's
  `pila-tty.log.old` in the repo root afterwards. The user's own config lives outside the
  repo (`~/.config/pila`); keep `config.example.toml` valid (a test parses it).
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
systemd and Noctalia's greeter, but the code and the host-side scripts (`scripts/`, `resources/session/`)
must also work elsewhere (Debian, Fedora, Void, Alpine, ...), including without systemd
(elogind or seatd, no per-user D-Bus bus, no journal): check for optional tools
(`systemd-cat`, `logger`, `dbus-run-session`) and degrade, use `/usr/bin/env bash`, and keep
install paths configurable. Name packages generically in messages ("package dbus"), not
with one distro's package manager. `rust-version` in `Cargo.toml` is the oldest Rust that
builds Pila; raise it when a dependency or the code needs a newer one. The dev container
itself is Arch, and its packages are named as Arch's.

## Performance pillar

Pila never makes an output miss a refresh while there is something to show (an
animation, a drag, a client's new frame), at the output's own refresh rate (60, 144 Hz,
...); 60 Hz is the floor it must hold everywhere. It renders on demand: smooth doesn't
mean redrawing when nothing changed. How fast clients draw is up to them.

- **Budget:** the CPU time Pila spends on a frame (scene, elements, submitting the
  render) stays well inside the period. Target: release build on the user's laptop (AMD
  Renoir), typical scenes, **at most 4 ms per output** (a quarter of 60 Hz, still room at
  144 Hz).
- **Measure it:** `RUST_LOG=pila::frames=debug` (`frame_stats.rs`) logs per output,
  every 5 s, frames, average and worst CPU time and missed refreshes, plus every frame
  over half the period, and apart the redraws that found nothing new to show (`idle`, with
  their summed CPU time; `scripts/check` reports them per output). Missed refreshes are vblanks with no new frame while an
  animation runs: exact on bare metal (the DRM sequence of the vblanks that showed
  frames) and an estimate nested (gaps over 1.5 periods; the host decides when frames
  show); `pila::frames=trace` logs every frame. The first frame after an output is set up
  (connected, powered on, a new mode, back from another VT: swapchain and modeset) is a cost of
  setting it up, logged on its own and not counted. Screen captures, which build frames of their
  own after the outputs', are logged apart (`captures`), held to the same budget (`scripts/check`
  streams one, as a screen share does: `scripts/capture-stream`). Any change that touches rendering, input
  or the event loop is checked with it (release build) before it's committed; bare-metal
  numbers come from the user with `scripts/check` (see Commands).
- **Rules:** nothing blocking on the event loop (I/O, D-Bus, font loading go to threads
  or run in the background, like `text.rs` and `export_environment`); work proportional
  to damage; expensive paths (offscreen textures, blur, captures) only while they're
  needed; frames paced by vblank (udev) or the host's frame callbacks (nested), never by
  timers (the headless test backend, which has no vblank, is the one exception;
  `schedule_hidden_frames`' and `commit_pacing.rs`'s once-a-second wakes for hidden
  surfaces, the wake for a timestamp a client asked for, the frame callbacks a captured
  hidden window gets at its capture client's rate and, on bare metal, the wait of an output
  that just redrew with nothing new until half a period before its next vblank, for the next
  redraw that may bring nothing either (`udev/draw.rs::idle_wait`: a client committing no
  buffer in a loop of frame callbacks would make it redraw as fast as it loops; a redraw
  with something new never waits) aren't pacing); redraws per output: a change redraws
  only the outputs it shows on (`redraw.rs`); idle callbacks
  inserted outside an event source come with a `Pila::wake` ping (see "Architecture");
  one-time costs are paid at startup, not in the
  first frame that needs them (`render::warm_up` when a renderer is created: shaders,
  an offscreen texture; `text::init` shapes and rasterizes common characters on its
  thread before publishing the fonts).

## Protocol policy

Pila prefers **standard protocols**: Wayland core and the official `wayland-protocols`
(stable, staging, and unstable where nothing newer exists). Where both exist, prefer the
`ext-`/staging protocol over an older unstable or vendor one (e.g. `ext-data-control`, not
`zwlr_data_control`). A standard protocol that Smithay doesn't implement (e.g.
`ext-image-copy-capture`) is implemented in Pila on the bindings from the
`wayland-protocols` crate, not swapped for a vendor one. (One temporary exception:
xdg-session-management's XML is in that crate's release but no module yet, so
`session_protocol.rs` generates its bindings with `wayland-scanner`; delete it, its
client copy in `tests/common/` and the XML when a release has them.)

A **wlroots protocol** is accepted when there's no standard equivalent yet and a desktop
tool the user runs needs it; each is replaced as soon as an `ext-` version lands in
`wayland-protocols` and Smithay. The accepted ones (all needed by panels/shells such as
Noctalia): `wlr-layer-shell`, `wlr-screencopy`, `wlr-foreign-toplevel-management`,
`wlr-gamma-control`, `wlr-output-management`, `wlr-output-power-management` and
`virtual-keyboard-unstable-v1`. Other vendor protocols (KDE, GNOME, Hyprland, …) are not
added, even if Smithay implements them. Features with no protocol at all go through
Pila's own config/IPC.

Privileged protocols are hidden from sandboxed apps: a client that connected through
wp-security-context (Flatpak) fails `state::is_privileged` (`ClientState::security_context`;
Xwayland counts as privileged). Smithay globals take it as their filter (those Smithay gives none,
keyboard-shortcuts-inhibit and ext-idle-notify, are made again hidden with
`state::hide_from_sandboxes`), Pila's own as
`GlobalDispatch::can_view`. It covers layer-shell, session lock, ext-data-control,
virtual-keyboard, ext-foreign-toplevel-list, screen capture (both protocols), toplevel
management, gamma, output management and power, ext-workspace, keyboard-shortcuts-inhibit,
ext-idle-notify, and security-context itself. A new
global that exposes or controls other clients, input or outputs must get it too.

## Architecture

One state struct, `Pila` (`src/state.rs`), implements every Smithay handler trait and is
the calloop event loop's data. Core window management lives in `state.rs`; each feature
beyond it has its own module, and `render.rs`, `frame.rs` and `post_render` are shared by
every backend.

**[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) describes every flow and invariant, a
section per subsystem. Read a subsystem's section before changing it** (they start with
its name and module: grep for them). A change to a flow or invariant updates its section
in the same commit, and a new feature adds one.

What breaks silently anywhere in the code, whichever subsystem you're in:

- **Redraws are on demand, per output** (`redraw.rs`): nothing redraws an output unless
  asked. A change asks for the outputs it shows on (`queue_redraw_output`,
  `queue_redraw_window`, `queue_redraw_rect`; commits and input already do), `queue_redraw`
  only for what's global; a change to the `Space`'s windows goes through `map_window`/
  `unmap_window`/`raise_window`. The tests' headless check (`PILA_HEADLESS_CHECK_REDRAWS`)
  panics on an output that changed with no redraw asked. An idle callback inserted outside
  an event source must ping `Pila::wake`, or it runs after the next event, however late;
  redraws requested from `after_frames` go through `defer_redraws`.
- **Session lock:** any new input or focus path must respect `is_locked()`.
- **Input:** a new input path goes through `process_input_event` (or calls
  `notify_activity`) to count as activity, and hit testing through `Pila::under`, which
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
- **Stacking:** a new way to raise a window must end in `Pila::keep_on_top` (`stacking.rs`), or
  always-on-top windows end up below it.
- **Grabs:** `Pila::set_cursor` must be called after `set_grab` (`Focus::Clear` resets the
  cursor).
- **Backends:** code outside them uses `Backend`'s methods rather than matching on it.
- **Settings:** a new one is read when used or re-applied in `apply_config`. Keep in sync:
  `DEFAULT_BINDINGS` and `DEFAULT_GESTURES`, the tables in `config.rs`'s module doc, the
  defaults in `config.example.toml` (every value commented out and marked default or example),
  `USAGE` in `main.rs` for flags and env vars, and the user docs: `docs/MANUAL.md` (by task)
  and the man page `docs/pila.1` (tests check that both list every default binding, and the
  man page every flag and variable of `USAGE`). A feature users see gets its place in the
  manual.

## Reference code

Smithay's `smallvil` (minimal stacking compositor) and `anvil` (full-featured) examples are
the canonical references. Clone `https://github.com/Smithay/smithay` outside `/workspace`
(e.g. `/tmp`) at the `rev` of the `smithay` dependency in `Cargo.toml` and read them before
inventing new patterns. Moving `rev` forward is a change of its own: read Smithay's
`CHANGELOG.md` and log since the old one, run the whole suite and `scripts/check --nested`,
and have the user check bare metal (`udev/`).
