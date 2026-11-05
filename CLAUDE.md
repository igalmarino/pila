# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# pila: a stacking Wayland compositor

A daily-driver **stacking** Wayland compositor (crate/binary name `pila`) written in **Rust**
on **Smithay 0.7.x**.

## Environment

- You run inside a dev container (Arch Linux base). The host is Arch Linux; the user logs
  into pila itself through GDM (`scripts/install`), with GNOME kept only as a fallback
  session.
- The compositor is developed **nested**: Smithay's winit backend opens it as a window on
  the host desktop. `WAYLAND_DISPLAY` points at the host socket for that purpose
  (`/run/host-wayland/wayland-0`, a hard link `scripts/link-wayland` makes to the socket of
  whichever session the container was entered from: pila, or GNOME); the compositor
  creates its own socket in `$XDG_RUNTIME_DIR` and clients launched from it connect there.
- The DRM/KMS + libinput backend (`--backend udev`, `udev.rs`) cannot be tested in this
  container (no seat, no TTY); there it exits with "could not start a session". It needs
  the host, from a TTY. Write that code, but ask the user to test it.
- You can't see the screen. Ask the user to confirm anything visual.
- No root and no package manager access. If a system library or tool is missing, say
  which Arch package is needed so the user can add it to `.devcontainer/Dockerfile`.
- Outbound network is allowlisted (crates.io, GitHub, Anthropic, docs.rs). Other hosts fail.
  New domains go in `.devcontainer/allowed-domains.txt` (user must rebuild the container).
  The firewall allows IPs; CDN-hosted domains (crates.io, docs.rs) rotate theirs, and a
  background loop adds new ones every 30s. A `cargo` "could not connect" usually clears after
  ~30s; if not, ask the user to run `! sudo /usr/local/bin/init-firewall.sh --refresh`.

## Commands

- Build: `cargo build`; `cargo build --release` for what `scripts/tty` and
  `scripts/install` use.
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt`
- Test: `cargo test`; single test: `cargo test <name_substring>`. Unit tests cover pure
  logic (e.g. title bar/border hit math in `decoration.rs`, config parsing, including
  `example_config_is_valid` for `config.example.toml`). Integration tests (`tests/*.rs`,
  harness in `tests/common/`) run a headless pila per test (`--backend headless`, see
  "Backends"), in parallel: `TestPila` starts it with a config (animations off unless
  the test sets `[animations]`), injects input and reads the state through its control
  socket, and waits for conditions (`wait`, `wait_window`, `stays`; never sleeps);
  `Client` (`tests/common/client.rs`) is a synthetic Wayland client on its own thread
  that acks configures, draws its color at the configured size and records configures
  and input events. A failing test prints the state and pila's log; `PILA_TEST_LOG`
  sets its `RUST_LOG`, `PILA_TEST_KEEP=1` keeps a failed test's directory,
  `PILA_HEADLESS_DEVICE=software` renders on llvmpipe as CI does. A pila that
  panics fails its test. Every feature gets integration tests; only bare-metal
  paths (`udev.rs`) and looks stay with `scripts/check` and the user.
  `tests/FEATURES.md` maps every feature to its tests (or says why it has
  none), and `features_inventory_is_complete` fails if a test it names is gone
  or an integration test isn't named there: a new test goes into it. Known
  bugs are `#[ignore = "bug: ..."]` tests, listed at its end. The client's
  `wait` closures run with its lock held: read the `Shared` given, never call
  the client (that deadlocks). `scripts/coverage [--html]` reports line
  coverage per file (instrumented build in `target/coverage`; needs
  `llvm-profdata`/`llvm-cov` of rustc's LLVM major version), a guide to what
  the tests miss.
- Run nested: `cargo run` opens a window on the host desktop and blocks. Don't leave it
  running in the foreground; use `timeout 10 cargo run` for smoke tests or ask the user.
  `cargo run -- -c '<cmd>'` runs `<cmd>` at startup instead of the terminal.
- Config: `config.example.toml` (copy to `~/.config/pila/config.toml`, or `--config`);
  `pila --check-config [--config FILE]` validates a file as pila reads it, plus the
  keymap (`config::check_keymap`), and exits 0/1 (`main::run_config_check`).
  Env overrides: `PILA_MOD` (super / alt / ctrl; default super on a TTY, alt nested),
  `PILA_TERMINAL` (default: the first installed of `config::TERMINALS`, kitty first),
  `PILA_LAUNCHER` (Mod+D, disabled if unset). Alt is the default because GNOME grabs Super
  while nested. On a TTY: `XCURSOR_THEME`/`XCURSOR_SIZE`, and the keymap from libxkbcommon's
  `XKB_DEFAULT_LAYOUT`/`_VARIANT`/`_OPTIONS`.
- Test clients available inside the nested compositor: `foot`, `weston-terminal`,
  `weston-simple-egl`, `wayland-info`, and `gtk4-widget-factory` / `gtk4-demo` (menus,
  popovers, dropdowns: the way to exercise popups and popup grabs). `foot` and GTK use
  cursor-shape (named cursors); `weston-terminal` draws its own cursor surfaces.
  Desktop tools for layer-shell and friends: `swaybg` (Background), `waybar` (Top, exclusive
  zone), `fuzzel` (Overlay, exclusive keyboard; `PILA_LAUNCHER=fuzzel`), `mako` +
  `notify-send` (notifications), `swaylock` (session lock), `wl-copy`/`wl-paste`
  (ext-data-control), `grim` + `slurp` (screenshots: ext-image-copy-capture). X11 (via
  Xwayland): `xterm`, `xeyes`, `xdpyinfo`.
- Automated checks: `scripts/check` runs scripted scenarios (windows, a window rule,
  minimize, a launcher, a layer rule via reload, Noctalia if installed) inside a pila
  with its own config and judges them from the log: events on `pila::animation`
  (`starts` / `off by rule`, with `what` and the app id or namespace) and `pila::rules`,
  plus the frame statistics (no missed refresh, CPU per frame on average within 4 ms).
  It prints PASS/FAIL per check and exits 1 on a failure. `scripts/check --nested`
  (release build, log in `pila-check.log`) is for you: run it before asking the user
  to check anything; the user runs plain `scripts/check` from a TTY (log in
  `pila-tty.log`) for bare metal. A new feature adds its scenario (a `mark "name
  begin/end"` section in the workload) and its checks (`section`, `has`, `lacks`),
  logging what it does on a debug target the checks can read; what only eyes can judge
  (how it looks) is still asked of the user.
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
- Arch package: `packaging/aur/pila-git` (`PKGBUILD` + `.SRCINFO`, test with `makepkg -f`
  there); its `package()` runs `scripts/install --no-build` with `PREFIX=/usr DESTDIR=`, so
  files added to `install_files` need no change there. Keep `depends` in step with the
  libraries pila links (`readelf -d`) and README's package table, and regenerate
  `.SRCINFO` (`makepkg --printsrcinfo`) after editing the `PKGBUILD`. The user publishes
  it to the AUR.
- TTY runs (the user, on the host): `scripts/tty [config]` runs the release build
  (`--debug` for the debug one) by hand on a spare VT, under `dbus-run-session`, while the
  login session keeps running; read `pila-tty.log` and the previous run's
  `pila-tty.log.old` in the repo root afterwards. The user's own config lives outside the
  repo (`~/.config/pila`); keep `config.example.toml` valid (a test parses it).
- This container is connected to the user's pila session, so `cargo run` opens a nested
  window inside it. Changes that need a pila restart are tested by the user.
- Git: small commits straight to `main`, after the user has checked anything visual or
  hardware-related; push to GitHub (`origin`, public, GPL-3.0-or-later) only when asked.

## Language

Everything in the repository is in English: code, identifiers, comments, docs, log
messages, test names, config examples and commit messages, even when the conversation
isn't.

## Distro policy

pila must not assume a distro, init system or display manager. This machine is Arch with
systemd and GDM, but the code and the host-side scripts (`scripts/`, `resources/session/`)
must also work elsewhere (Debian, Fedora, Void, Alpine, ...), including without systemd
(elogind or seatd, no per-user D-Bus bus, no journal): check for optional tools
(`systemd-cat`, `logger`, `dbus-run-session`) and degrade, use `/usr/bin/env bash`, and keep
install paths configurable. Name packages generically in messages ("package dbus"), not
with one distro's package manager. `rust-version` in `Cargo.toml` is the oldest Rust that
builds pila; raise it when a dependency or the code needs a newer one. The dev container
itself is Arch, and its packages are named as Arch's.

## Performance pillar

pila never makes an output miss a refresh while there is something to show (an
animation, a drag, a client's new frame), at the output's own refresh rate (60, 144 Hz,
...); 60 Hz is the floor it must hold everywhere. It renders on demand: smooth doesn't
mean redrawing when nothing changed. How fast clients draw is up to them.

- **Budget:** the CPU time pila spends on a frame (scene, elements, submitting the
  render) stays well inside the period. Target: release build on the user's laptop (AMD
  Renoir), typical scenes, **at most 4 ms per output** (a quarter of 60 Hz, still room at
  144 Hz).
- **Measure it:** `RUST_LOG=pila::frames=debug` (`frame_stats.rs`) logs per output,
  every 5 s, frames, average and worst CPU time and missed refreshes, plus every frame
  over half the period. Missed refreshes are vblanks with no new frame while an
  animation runs: exact on bare metal (the DRM sequence of the vblanks that showed
  frames) and an estimate nested (gaps over 1.5 periods; the host decides when frames
  show); `pila::frames=trace` logs every frame. Any change that touches rendering, input
  or the event loop is checked with it (release build) before it's committed; bare-metal
  numbers come from the user with `scripts/check` (see Commands).
- **Rules:** nothing blocking on the event loop (I/O, D-Bus, font loading go to threads
  or run in the background, like `text.rs` and `export_environment`); work proportional
  to damage; expensive paths (offscreen textures, blur, captures) only while they're
  needed; frames paced by vblank (udev) or the host's frame callbacks (nested), never by
  timers (the headless test backend, which has no vblank, is the one exception;
  `schedule_hidden_frames`' once-a-second wake for hidden surfaces isn't pacing); idle callbacks inserted outside an event source come with a `Pila::wake`
  ping (see "Redraws are on demand"); one-time costs are paid at startup, not in the
  first frame that needs them (`render::warm_up` when a renderer is created: shaders,
  an offscreen texture; `text::init` shapes and rasterizes common characters on its
  thread before publishing the fonts).

## Protocol policy

pila prefers **standard protocols**: Wayland core and the official `wayland-protocols`
(stable, staging, and unstable where nothing newer exists). Where both exist, prefer the
`ext-`/staging protocol over an older unstable or vendor one (e.g. `ext-data-control`, not
`zwlr_data_control`). A standard protocol that Smithay doesn't implement (e.g.
`ext-image-copy-capture`) is implemented in pila on the bindings from the
`wayland-protocols` crate, not swapped for a vendor one.

A **wlroots protocol** is accepted when there's no standard equivalent yet and a desktop
tool the user runs needs it; each is replaced as soon as an `ext-` version lands in
`wayland-protocols` and Smithay. The accepted ones (all needed by panels/shells such as
Noctalia): `wlr-layer-shell`, `wlr-screencopy`, `wlr-foreign-toplevel-management`,
`wlr-gamma-control`, `wlr-output-management`, `wlr-output-power-management` and
`virtual-keyboard-unstable-v1`. Other vendor protocols (KDE, GNOME, Hyprland, …) are not
added, even if Smithay implements them. Features with no protocol at all go through
pila's own config/IPC.

Privileged protocols are hidden from sandboxed apps: a client that connected through
wp-security-context (Flatpak) fails `state::is_privileged` (`ClientState::security_context`;
Xwayland counts as privileged). Smithay globals take it as their filter, pila's own as
`GlobalDispatch::can_view`. It covers layer-shell, session lock, ext-data-control,
virtual-keyboard, ext-foreign-toplevel-list, screen capture (both protocols), toplevel
management, gamma, output management and power, and security-context itself. A new
global that exposes or controls other clients, input or outputs must get it too.

## Architecture

Structure follows Smithay's `smallvil`. One state struct, `Pila` (`src/state.rs`),
implements every Smithay handler trait. Unlike smallvil (but like anvil), the calloop event
loop's data is `Pila` itself (`EventLoop<'static, Pila>`, `Pila::loop_handle`), which
Smithay modules that schedule their own timers (e.g. idle-notify) require. Core window
management lives in `state.rs`; each feature beyond it has its own module (named in the
flows below).

Key flows and invariants:

- **Configuration** (`config.rs`): a TOML file (`~/.config/pila/config.toml` or
  `--config`, documented in `config.example.toml`) parsed by `Config::from_toml` (pure,
  unit-tested; `PILA_*` env vars override it), loaded at startup by `Config::load`, which
  falls back to defaults on errors, and reloaded when the file changes
  (`config_reload.rs`: inotify on its directory, a 250 ms settle timer, then
  `Config::read`; an invalid file keeps the running config and notifies). `apply_config`
  re-applies what changed: keyboard (`set_xkb_config`), rules (`refresh_rule_props`),
  `[input]` on connected devices
  (`UdevData::apply_input_config`), `[[output]]` (`Pila::apply_outputs` →
  `udev::apply_output_config`: enabled, mode via `use_mode`, scale, transform, position,
  adaptive sync; nested only scale) with wlr-output-management's runtime overrides on
  top (`Pila::output_overrides`; an edited section drops its monitor's). Anything that reads
  `self.config` when used (bindings, Mod, terminal) needs nothing; `autostart` isn't
  re-run. A new setting must either be read when used or be re-applied there. Keep in
  sync: `DEFAULT_BINDINGS` and `DEFAULT_GESTURES`, the tables in `config.rs`'s module doc
  and the defaults listed in `config.example.toml`; and the `USAGE` string in `main.rs`
  with the CLI flags and env vars.
- **Keybindings** are intercepted in the keyboard filter closure (`input.rs`), which looks
  up every key press in `Config::bindings` (modifiers must match exactly, Caps/Num Lock
  ignored; keys without Mod, e.g. media keys, can be bound); intercepted keycodes go in
  `suppressed_keys` so their release isn't forwarded to the client either.
  Bindings don't run while the focused surface (or its menu's window) holds an active
  keyboard-shortcuts inhibitor (`shortcuts_inhibited`; all requests are granted), or
  while the session is locked; Quit is the exception to the inhibitor (and, in debug
  builds, to the lock).
- **Backends** (`backend.rs`): `Pila::backend` is a `Backend` enum, `Winit` (nested,
  `winit.rs`), `Udev` (DRM/KMS + libinput + libseat on a TTY, `udev.rs`) or `Headless`
  (tests, `headless.rs`), chosen by `--backend` or by whether `WAYLAND_DISPLAY`/`DISPLAY`
  is set. Code outside the backends uses `Backend`'s methods (renderer for dmabuf
  imports, `queue_redraw`, `cursor_changed`, LEDs, VT switch) rather than matching. All
  feed the same `scene()`, `render.rs`, `frame.rs` and `post_render`. `Pila::new`
  hard-codes a keyboard and pointer for all.
- **Headless (tests):** virtual outputs (`--headless-outputs NAME:WxH[@HZ],...`; each
  also offers 1024x768 and 1920x1080) rendered into offscreen textures on a render node
  or Mesa's software device, paced by a timer at their refresh rate while a redraw is
  queued; `[[output]]` applies in full (`headless::apply_output_config`, with
  wlr-output-management's overrides), gamma and power are kept in memory. Mod defaults
  to Super and nothing starts without `-c`/autostart. The control socket
  (`test_control.rs`, `$XDG_RUNTIME_DIR/<WAYLAND_DISPLAY>.control`, JSON lines; its
  commands are in the module doc) injects `TestInput` events through
  `process_input_event` (pointer moves are relative motion), reads pixels of the last
  frame, adds/removes outputs, and answers `state` with `Pila::state_json`
  (`introspect.rs`: windows with geometry, stacking, workspace, fill/snap state,
  decorations and rule props; focus, Mod+Tab, layers, outputs, pointer, lock,
  animations). A new feature exposes there what its tests need to check.
- **Nested-only (winit):** named cursor shapes are drawn by the host (`apply_cursor`);
  client cursor surfaces are drawn by pila (`cursor.rs`). Winit doesn't report the pointer
  leaving the window, so a drawn cursor lingers at the edge. Keys held when the host takes
  focus away (e.g. GNOME's Alt+Tab) never get a release (Smithay's winit backend drops
  winit's synthetic ones), so `WinitEvent::Focus(false)` calls `Pila::release_all_keys`.
  The dmabuf global is created in `winit::init_dmabuf`. Input is absolute motion only,
  mapped onto the single output; relative motion is derived from it.
- **Bare metal (udev):** one GPU (the seat's primary), outputs placed by their
  `[[output]]` config (mode, scale, position, transform) or else left to right, EDID off
  (make/model "Unknown", see `Cargo.toml`). pila draws named cursors from the xcursor
  theme (`cursor::CursorTheme`). Redraws are per output: `queue_redraw` schedules one idle
  `render_all`; an output with a frame in flight renders at its vblank (`frame_finish`),
  which also sends presentation feedback with the kernel's timestamp. The session lock is
  confirmed once every output has *shown* a locked frame. `PauseSession` (VT switch away)
  releases held keys. Libinput devices get keyboard LEDs and the
  `[input.touchpad]`/`[input.mouse]` settings (`configure_pointer`; a touchpad is a device
  that can tap) when added; relative motion goes through the same constraint path as
  nested (`pointer_motion`) and is clamped to the outputs. A removed output goes through
  `Pila::output_removed` (closes its layer surfaces, drops its lock surface, moves the
  pointer and stranded windows, incl. their restore geometry, via `fixup_windows`).
  `disable-internal-display` (a boolean; `Config::internal_display`: never / always)
  turns a built-in screen (connector eDP/LVDS/DSI, `is_internal`) off while another,
  external output is on by its own config: `effective_output_config` sets its `enabled`
  to false unless its `[[output]]` sets `enabled` (overrides still on top), decided from
  config rather than running outputs, counting connectors the scan in progress found
  but hasn't set up yet (`UdevData::scan_pending`), so at startup the built-in screen is
  never lit. An output off by the rule (`off_by_rule`) keeps its place in the
  left-to-right layout (`connector_connected`, `apply_output_config`), so it doesn't
  overlap when it comes back. `apply_internal_display` applies it (with a
  never-zero-outputs fallback) at the end of `scan_connectors`, in
  `connector_disconnected` before the gone output is unmapped (so its windows move
  onto the built-in screen); config reloads (of `[[output]]` or of this key) and
  wlr-output-management go through `apply_output_config`, which enables outputs before
  disabling any, decides held built-in screens last and keeps the same fallback
  (`others_enabled`, `keep_on`); a changed rule clears built-in screens' `enabled`
  overrides, which display tools set for every head. There's
  no lid handling: a clamshell mode was tried and dropped, since logind suspends a
  closed laptop while no external output is on (e.g. between sessions).
  `fullscreen_request` honors the requested output for mapped windows
  (`set_fill_state_on`). Touchpad gestures (swipe, pinch, hold) are forwarded to the
  pointer focus through pointer-gestures (`gestures.rs`), except swipes with a finger
  count that `[gestures]` binds: those are kept whole from begin to end (`Pila::swipe`,
  direction fixed after a small move, action after a threshold, window switching stepping
  per further stretch and ending on lift), unless locked or shortcuts are inhibited.
  Output transforms (rotation) are the `Output`'s: Smithay rotates the rendered frame and
  the `Space` swaps a turned output's logical size, so pila only sets it (and sizes output
  captures accordingly); clients are told `Normal` and draw upright. Still missing: touch.
- **Render loop** (`winit::redraw`): collects `Pila::scene()` (windows front to back with
  their title bars) and the cursor *before* borrowing the backend, builds elements with
  `render.rs` (backend-neutral, but on `GlesRenderer`: `PilaRenderElement` is concrete,
  since pila's custom shaders need `GlesFrame`), and calls the damage tracker's
  `render_output` directly. Smithay's `space::render_output` isn't used because it can't
  interleave title bars with windows. Afterwards `frame.rs` (backend-neutral) records each
  drawn surface's primary output from the render states, sends presentation-time feedback
  once the frame is submitted, and sends frame callbacks only to surfaces whose primary
  output it is (surfaces shown nowhere: once a second, `HIDDEN_FRAME_THROTTLE`, so a
  covered animating client doesn't spin; `schedule_hidden_frames`, from `after_frames`,
  asks for a redraw after that second while such a surface waits, since nothing
  else may redraw); frame callback times and
  feedback both use `Pila::clock` (CLOCK_MONOTONIC). Once per event-loop
  iteration, however many outputs were drawn, `after_frames` (`main.rs`) runs captures,
  screencopies, `space.refresh()`, taskbar updates and `popups.cleanup()`, then a flush. Buffer age (for partial repaints) needs the window's EGL
  surface made current first (`winit::make_surface_current`); Smithay's `bind` is lazy,
  and querying without it fails with `EGL_BAD_SURFACE` every animated frame.
- **Redraws are on demand** (`Pila::queue_redraw`): after every client dispatch (display
  source), every winit event, while a resize needs full frames or a lock awaits its
  first locked frame, and while an animation runs. Nothing else redraws, so state changes
  outside those paths (timers, `insert_idle` callbacks) must call `queue_redraw`
  themselves. Clients are flushed after every event-loop iteration (`main.rs`), not only
  on redraw. calloop doesn't wake the loop for pending idle callbacks: they run after
  the next event, however late. So `queue_redraw` pings `Pila::wake` (bare metal, the
  redraw itself is an idle `render_all`), and anything else inserting an idle callback
  outside an event source (e.g. from `after_frames`, the `event_loop.run` callback) must
  ping it too. A redraw requested from `after_frames` goes through `insert_idle` plus
  the ping: nested, the winit source has already checked for redraw requests by then.
- **Pointer grabs:** Mod+drag installs the move/resize grab *before* the press is
  forwarded, so the client never sees that click. Move/resize grabs show their own cursor
  via `Pila::set_cursor`, which must be called *after* `set_grab` (`Focus::Clear` resets the
  cursor), and reset it in `PointerGrab::unset`. Client-requested move/resize goes through
  `check_grab` (`handlers/xdg_shell.rs`), which requires a live implicit grab on that
  client's surface. Popup grabs (menus, `XdgShellHandler::grab`) use Smithay's
  `PopupKeyboardGrab`/`PopupPointerGrab`; they are the only keyboard grabs pila takes.
  A popup grab ignores focus changes, so `focus_window` dismisses it first
  (`dismiss_popup_grab`), and `focused_window` resolves a focused popup to its window.
- **Window lifecycle:** `new_toplevel` pushes the window onto `Pila::unmapped` (not the
  space). A client unmapping it (a null-buffer commit, `unmap_if_buffer_removed`) takes it
  out of the space, taskbars (`remove_foreign_toplevel`, which also stops its capture
  sessions), Mod+Tab and focus, and puts a new `Window` for the toplevel in `unmapped`:
  it maps again like a new window. `handle_xdg_commit` sends the initial configure, then on the first commit with a
  buffer moves it into `space` via `place_new_window` (centered on the pointer's output,
  cascaded by `CASCADE_STEP` if the spot is taken). Lookups that must find unmapped windows
  use `window_for_surface`, which searches both.
- **Stacking order** is the `Space` element order (back to front); raising = `raise_element`.
  `focus_window` both raises and sets keyboard focus, then flushes pending configures.
- **Window switching** (`switcher.rs`): `focus_window` moves the window to the front of
  `Pila::focus_history` (most recent first) unless a Mod+Tab is in progress (`Pila::switch`,
  a snapshot of that order). `cycle-focus`/`cycle-focus-back` step through the snapshot;
  the switch ends (`end_switch`, which records the choice) when Ctrl/Alt/Super are all
  released (checked after every key in `on_keyboard`), on a click, and in
  `release_all_keys`. Unmapped windows are dropped with `forget_focus_history`.
- **Minimize** (`minimize.rs`): a minimized window is unmapped from the `Space` and kept
  in `Pila::minimized` with its location (X11 ones get `_NET_WM_STATE_HIDDEN`).
  `window_for_surface`/`window_for_x11` search it too, and destroyed windows are removed
  (`forget_off_space`). It comes back only through Mod+Tab: the switch order includes
  minimized windows; a minimized pick is shown (`show_window`) while selected, hidden
  again (`hide_window`) when the switch moves on, and stays when the switch ends.
- **Workspaces** (`workspaces.rs`, `config.workspaces` of them, spanning all outputs): the
  `Space` holds only the active workspace's windows (and X11 override-redirect ones);
  the others' are parked in `Workspaces::parked` (back to front, workspace and location in
  user data, X11 ones suspended). Minimized windows remember their workspace too, so
  "off the `Space`" means `off_space_windows()` (parked + minimized): lookups by surface,
  X11 surface or identifier and taskbar listeners chain it, and destroyed windows are
  dropped from both (`forget_off_space`); both keep location and workspace in the same
  user-data record (`workspaces::leave_space`/`space_location`). Invariant: a window in the `Space` is on the
  active workspace. `switch_workspace` (refused while locked) ends Mod+Tab and popup grabs,
  swaps the windows (a window in pila's move/resize grab comes along; other pointer
  grabs, e.g. a client's DnD, are left alone), focuses the most recent one there and
  refreshes pointer focus. Mod+Tab only lists the active workspace; activating a window elsewhere
  (taskbar, xdg-activation) switches first (`reveal_workspace_of`). ext-workspace
  (`ext_workspace.rs`, on the `wayland-protocols` bindings): one group with every
  output, workspaces "1".."N", only `activate`, applied on `commit`; diffed by
  `refresh_workspaces` (after a switch, a count change, from `refresh_output_heads` and
  on a client's `wl_output` bind via `OutputHandler::output_bound`).
- **Layer shell** (`layers.rs`, `handlers/layer_shell.rs`): layer surfaces live in each
  output's `LayerMap`, not the `Space`. Front to back: Overlay, Top, windows, Bottom,
  Background; a fullscreen window on top also covers Top (`layer_order`), unless a
  mapped Top surface takes the keyboard exclusively (`takes_keyboard`). All pointer
  hit testing goes through `Pila::under` (layers and windows in that order; `window_under`
  is windows only), and `Pila::scene` renders the same order. Placement, maximize and
  toplevel bounds use `usable_geometry_at` (the output minus exclusive zones); fullscreen
  uses the whole output. When the zone or output size changes, `refit_maximized` re-applies
  fill states. A mapped Top/Overlay surface with exclusive keyboard interactivity
  (`exclusive_layer`) keeps keyboard focus: `focus_window` then only raises. Popups may
  have a layer-surface root (`unconstrain_popup`, popup `grab`).
- **Simple protocols** (viewporter, fractional-scale, data-control, xdg-foreign, …) live in
  `handlers/protocols.rs`: globals are created in `init_globals`, and only state a handler
  must return is kept (`Pila::globals`).
- **Animations** (`animation.rs`, `[animations]` in the config:
  `config::AnimationsConfig`, all on by default, each a duration in ms or `false`, plus
  a global `enabled` and `speed`): short and subtle. So far a window rises and fades in
  when mapped (`animate_open` from `place_new_window`; not X11 override-redirect),
  shrinks, drops and fades out when it goes (`animate_close`), and shrinks away a little lower
  when minimized (`animate_minimize` from `hide_window`, before the unmap) and back when
  restored (`animate_restore` from `show_window`): a `Style` (`Open`, `Minimize`,
  `Layer`) gives each its look. A workspace switch slides (`workspace_slide.rs`):
  `start_workspace_slide` (from `show_workspace`, before the windows are swapped) draws
  each output's windows into a texture; while it runs `render::slide_elements` draws
  the new workspace's windows into another per-output texture (`SlideTextures`) and
  shows both shifted sideways and cropped to their output (`slid_texture`), the carried
  window plainly in front; panels and wallpaper stay put; windows drawn that way are
  paced by `SceneWindow::slid` (`offscreen_output` in `frame.rs`; on no output, by
  their primary output). A switch during a slide takes its leaving picture from what
  the running slide shows (`snapshot_windows` with the slide). Close and minimize
  ghosts belong to the workspace they left (`Ghost::workspace`): a switch carries them
  off in the leaving picture. A size or place
  change (maximize, fullscreen, snap, and back; not refits nor drags) runs
  `animate_geometry` first (from `set_fill_state_on`, `snap_window`, `unsnap_window`;
  not while a window opens, nor inside `without_geometry_animation`, which
  `begin_move` uses): a `Resizing` keeps a snapshot and the old frame; until the
  client commits a new size (`geometry_committed` from the commit handler, by
  `committed_size`: an X11 window's buffer, since X applies a configure at once; or
  `RESIZE_WAIT`) the live window is hidden (alpha 0) and the snapshot stands at the old
  rect; then both are stretched onto a rect moving to the new one (`FrameMap`), the
  snapshot fading out in front. Textures are always drawn with their whole `src`
  (`texture_element`): without it Smithay samples a destination-sized corner, which
  crops a shrunk texture and repeats a stretched one. Window and layer rules can turn them off
  (`animates_window`, `animates_layer`; see "Window and layer rules"). Transient layer
  surfaces (Top/Overlay without an exclusive zone, `animates_layer`: launchers,
  notifications, shell panels; not bars or wallpapers) fade in when they first get a buffer (`layer_committed` from
  `handle_layer_commit`) and out from a snapshot (`animate_layer_close`: `watch_layer`
  hooks, `layer_destroyed`), drawn in front of the upper layers (`Scene::layer_ghosts`);
  their rect comes from the layer map, which outlives the surface tree. A blurred
  backdrop fades with its layer (`SceneLayer::alpha`). A new animation of a window starts from where one in progress left it
  (`appear`/`disappear`; Mod+Tab previews of minimized windows toggle quickly). Values come from `Animation`
  (from/to over a duration on `Pila::clock`, one time per `scene()`). An animating
  window's `SceneWindow::effect` (a layer's `SceneLayer::effect`) makes `render.rs` draw
  it (bar, popups, corners included) into a per-scale texture (`OffscreenWindow` in its
  `AnimTextures` user data, `animated_element`) and show that transformed; `frame.rs`
  paces it by `WindowEffect::output`, since it's absent from the output's render states. A closed
  window is a `Ghost`: a snapshot (`render::snapshot_window`) drawn at its old place in
  the stacking order (`Scene::ghosts`) while it fades; its live window is skipped
  meanwhile (`is_ghost`); one its client maps again before the fade ends is
  dropped and the window opens again (`geometry_committed`). The snapshot must be
  taken while the content exists:
  `watch_unmap` (added in `new_toplevel`) catches the commit that removes the buffer
  (pre-commit hook; most clients unmap before destroying) and the surface's destruction
  (a dead client: a destruction hook added before Smithay's, which drops the textures,
  so only the main surface is drawn, `Surfaces::MainOnly`, the tree being gone); X11
  windows are taken in `unmapped_window`. Windows leaving the space otherwise
  (workspaces) drop their animation (`forget_animations`). `scripts/toplevel-action`
  (a stdlib Python taskbar client) minimizes/unminimizes and `scripts/workspace-action`
  (ext-workspace) switches workspaces for scripted tests (`scripts/check`); `wtype`
  can't, the virtual keyboard bypasses bindings. `step_animations` (from
  `after_frames`) prunes finished ones and requests the next frame; locking drops all.
  Animations are visual only: input sees the real geometry.
- **Window and layer rules** (`rules.rs`, `[[window-rule]]` / `[[layer-rule]]`): match
  windows by app id (`foreign_toplevel::title_and_app_id`: xdg `app_id` or X11
  WM_CLASS class) and title, layer surfaces by namespace, with `*` wildcards
  (`wildcard_match`); all of a rule's matchers must match, matching rules merge in order
  (later wins per field). The result is resolved once and kept in user data
  (`resolve_window_rules` from `place_new_window`, `resolve_layer_rules` from
  `new_layer_surface`), read by `rules::window_props`/`layer_props`, and re-resolved for
  every window and layer surface on a reload that changes the rules
  (`refresh_rule_props` from `apply_config`): never matched per frame. A title change
  doesn't re-match. Window properties: `animations` (all or per kind), and, only when
  it opens: `size` and `open-maximized`/`open-fullscreen` before an xdg toplevel's
  initial configure (`apply_initial_rules` from `handle_xdg_commit`; X11: size by
  `configure`, fill after placing, in `map_window_request`), `position` (from the
  usable area's top-left, title bar included) and `open-on-workspace` (`park`ed there,
  unfocused, no animation) in `place_new_window`; live, like `animations`: `title-bar =
  false` (`decoration::bar_hidden`: no bar drawn nor laid out, the window stays SSD),
  `corner-radius` (over the global one, in `scene_window`) and `opacity`
  (`SceneWindow::alpha`, passed to every element of the window, title bar and flash
  included; Smithay's elements drop their opaque regions below 1). Hot paths read them
  through `rules::with_window_props` (no copy); a reload also re-fits filled windows
  (a bar shown or hidden moves them). Layer properties: `animations` and
  `blur` (`blur_rects`: `true` the whole surface, `false` none, unset the client's
  region). Each applied rule logs on `pila::rules` for `scripts/check`.
- **System bell** (`bell.rs`, xdg-system-bell): visual only. The ringing window (its root
  window for a subsurface or popup; the focused window when the client names none) gets a
  short `BellFlash` in its user data, which `scene()` turns into a solid element drawn just
  in front of that window and its title bar (`SceneWindow::flash`); a timer redraws when it
  ends. Ignored while locked.
- **Output scale** (`scale.rs`): per output from `[[output]]` (`Config::output`; nested
  the output is `winit`, scale only), may be fractional. Every coordinate pila handles is
  logical; `render.rs` gets the scale per output (`scale::output_scale`). pila's own
  textures (title bar buttons and text, xcursor images) are rasterized at the scale rounded
  up (`scale::buffer_scale`) and cached per scale. Clients get fractional-scale and
  `preferred_buffer_scale` (wl_compositor v6) after every frame (`update_surface_scales`
  from `post_render`), and before their first configure (`send_initial_scale`). A
  window's scale is that of `Pila::window_output`, the output holding most of its
  geometry: Smithay's per-frame primary output flips for a window straddling two
  outputs of different refresh rates, and a client told a new scale every frame
  redraws forever. Layer and lock surfaces use their own output. Xwayland stays at scale 1 (upscaled).
- **Screen capture** (`capture.rs`, ext-image-capture-source + ext-image-copy-capture, on
  the `wayland-protocols` bindings): sources are an output or a window (by its
  foreign-toplevel identifier). Sessions live in `Pila::captures`; frames are rendered
  from `scene()` (window sources: only the window, no title bar) straight into the
  client's dma-buf (device and modifiers from `Pila::capture_dmabuf`, set by the backend),
  or offscreen and read back into shm (GL reads rows bottom-up, so they're reversed). A
  session's first frame is copied at once (idle), later ones in `after_frames` only when
  the session's own damage tracker sees a change. Size changes re-send constraints and
  fail the frame; a gone source stops the session (`stop_captures_of`). While locked,
  window captures wait. Cursor sessions are created stopped; `paint_cursors` is honored.
- **Screencopy** (`screencopy.rs`, wlr-screencopy v3, for clients that predate
  ext-image-copy-capture): a frame names an output and optionally a region (logical,
  output-local); pila answers `buffer` (shm Xrgb8888), `linux_dmabuf` when captures may
  use dma-bufs, `buffer_done`. `copy` is rendered from the scene with `capture::copy_frame`
  in an idle callback; `copy_with_damage` waits in `Pila::screencopies` until that
  manager's damage tracker for the output/region (`screencopy_trackers`) sees a change,
  checked from `after_frames`. A mode/scale change or a removed output fails the frame.
- **Toplevel management** (`toplevel_management.rs`, wlr-foreign-toplevel-management v3):
  taskbars act on windows named by their ext-foreign-toplevel-list identifier
  (`foreign_toplevel::window_for_identifier`). Listeners are updated by diffing in
  `refresh_toplevels` (from `after_frames`, after `space.refresh()`): title, app id, states
  (Activated = keyboard focus, Minimized from `minimize.rs`, Maximized/Fullscreen from
  `fill_state`) and the outputs each client's handle was told; new windows get a handle
  per manager, destroyed ones `closed` (`remove_foreign_toplevel`). Requests map onto
  `focus_window`/`restore_window`, `minimize_window`/`show_window`, `set_fill_state(_on)`
  and `close_window`, and are ignored while locked.
- **Gamma** (`gamma_control.rs`, wlr-gamma-control): one control per output, ramps set
  through `Backend::set_gamma` (udev: the legacy `set_gamma` ioctl on the CRTC; remembered
  in `OutputSurface.gamma` and re-applied after `ActivateSession`, a mode change and a
  power on; nested: fails). Destroying the control restores linear.
- **Output power** (`output_power.rs`, wlr-output-power-management): off = drop the
  `OutputSurface.drm_output` (the CRTC is cleared, the monitor sleeps), on =
  `initialize_output` again with the stored connector and mode (`UdevData::set_output_power`).
  Code using `drm_output` must handle `None`; a powered-off output stays mapped and counts
  as showing the lock screen. Nested: fails.
- **Output management** (`output_management.rs`, wlr-output-management v4): heads are
  `HeadInfo` snapshots (`Backend::output_heads`; nested: the window, transform `Normal`,
  only its scale may change). `refresh_output_heads` diffs against the last snapshot,
  sends the changes and `done` with a new serial, which cancels configurations in
  progress; it runs after connect/disconnect, `apply_outputs` and a nested resize. A
  configuration is checked against the snapshot (`check_output_configuration`: known
  modes only, ≥ 1 head enabled, scale 0.5..8) and applied as `[[output]]`-style overrides
  in `Pila::output_overrides`, which `udev::effective_output_config` layers over the file
  for `connector_connected` and `apply_output_config`. Disabling a head
  (`disable_output`) clears its CRTC, removes its global and runs `output_removed`; the
  `OutputSurface` stays so the head is still listed; `enable_output` is the reverse (also
  used by `connector_connected`). Adaptive sync goes through `DrmCompositor::use_vrr`.
- **Blur** (`blur.rs`, ext-background-effect-v1): a surface's blur region is `Cacheable`
  surface state (`BlurRegion`, committed by Smithay); `Pila::scene` evaluates it per
  layer surface into `SceneLayer::blur` (upper layers only for now). Before an output's
  elements are built, both backends (and captures) call `BlurCache::prepare`: for each
  blurred layer it renders what's behind the region (`render::elements_behind_layer`)
  into an offscreen texture through a per-region damage tracker, and only when that
  changed runs the dual-Kawase passes (`BLUR_PASSES` levels, custom texture shaders,
  `Transform::Normal` throughout, textures with linear filtering and clamped edges);
  `BlurCache::element` gives `render::scene_elements` the result (a `Texture` variant of
  `PilaRenderElement`) to draw right behind that layer. A shader failure disables blur.
- **Session environment** (`Pila::spawn`, `start_session`): children get `WAYLAND_DISPLAY`,
  pila's Xwayland `DISPLAY`, `XDG_CURRENT_DESKTOP=pila` and `XDG_SESSION_TYPE=wayland`;
  `spawn` adds `PILA_OUTPUT`, the output under the pointer.
  Screen sharing goes through xdg-desktop-portal + xdg-desktop-portal-wlr (PipeWire),
  which `resources/xdg/xdg-desktop-portal/pila-portals.conf` selects; xdpw's own config
  (`resources/xdg/xdg-desktop-portal-wlr/pila`: pick the screen with slurp) must be in
  `~/.config/xdg-desktop-portal-wlr/` or `/etc/xdg` (xdpw ignores `XDG_CONFIG_DIRS`;
  `scripts/install` puts both in `/etc/xdg`). On a TTY, before the startup programs, pila
  exports that environment to the D-Bus session bus (`export_environment`): on the systemd
  user bus (a login session) also to `systemd --user`, which starts the portals and
  pipewire there (`--systemd`); on a private bus (`scripts/tty`, `dbus-run-session`) the
  bus alone, so a test instance leaves the login session's services alone. The export
  runs in the background (a stuck bus mustn't freeze pila); the startup programs follow
  when it's done or after `EXPORT_TIMEOUT`. Nested, apps use the host's portals. Spawned
  programs are kept in `Pila::children` and reaped on SIGCHLD (`reap_children`); the
  signal source is created first thing in `main`, before any thread, so no thread can
  take the signal instead.
- **Foreign toplevels** (`foreign_toplevel.rs`, ext-foreign-toplevel-list, read-only): a
  window's handle is created in `place_new_window`, kept in its user data, updated on
  commit and closed in `toplevel_destroyed`.
- **Session lock** (`session_lock.rs`, ext-session-lock): while `is_locked()`, `scene()`
  holds only lock surfaces (on black), `surface_under` returns only lock surfaces,
  `focus_window`/`focus_layer` keep the keyboard on a lock surface, popup and move/resize
  grabs are refused and bindings don't run (Quit still works in debug builds). Any new
  input or focus path must respect `is_locked()`. The lock is confirmed (`confirm_lock`)
  only after a full locked frame is submitted. Smithay 0.7 calls `unlock()` even for a
  rogue client's `unlock_and_destroy`, so `unlock` defers to `finish_unlock`, which only
  unlocks once the holder's lock object is gone. A dead locker leaves the session locked;
  a new locker may take over.
- **Idle** (`idle.rs`, ext-idle-notify + idle-inhibit): every input event calls
  `notify_activity`; Smithay runs the notification timers on `loop_handle`. Idle inhibitors
  only count while their surface is part of a mapped window or layer surface and the
  session isn't locked; `update_idle_inhibit` re-checks after every frame. A new input path
  must go through `process_input_event` (or call `notify_activity`) to count as activity.
- **Pointer constraints** (`pointer_constraints.rs`): a lock/confine activates only while
  the pointer is over its surface (inside its region) and that window has keyboard focus;
  `SeatHandler::focus_changed` releases it (`release_constraint_for_focus`), which makes
  Mod+Tab / clicking elsewhere / locking the escape. The motion path checks the active
  constraint before moving the pointer. Nested, relative motion is derived from the host
  pointer (`Pila::host_pointer`); a DRM backend sends real relative motion through the same
  checks.
- **X11** (`xwayland.rs`, `focus.rs`): pila starts Xwayland at launch and runs Smithay's
  `X11Wm`; spawned clients get its `DISPLAY` (never the host's) and the startup command
  waits for X (`pending_startup`). X11 windows are `Window::new_x11_window` in the same
  `Space`, so code that handles windows must not assume `window.toplevel()` is `Some`: use
  `wl_surface()`, `fill_state`, `close_window`, `focus_target`, and branch on
  `x11_surface()` where X11 differs. Keyboard focus is `focus::KeyboardFocus` (Wayland
  surface or X11 window, so Smithay sets the X input focus); compare surfaces through
  `Pila::keyboard_focus()`. Whenever pila moves a managed X11 window it must tell X
  (`x11_window_moved`); focusing raises it in X too. Override-redirect windows (menus,
  tooltips) go where X puts them. X11 maximize/fullscreen go through `set_fill_state`
  (`_NET_WM_STATE` + a direct configure); resize grabs configure X11 windows per motion.
  Selections are bridged both ways (`SelectionHandler` in `handlers/mod.rs`, XwmHandler
  selection methods); only a focused X11 app may read.
- **Snapping and keyboard moves** (`snap.rs`): a snapped window fills half of its output's
  usable area, gets xdg `Tiled*` states, and keeps its pre-snap rect in `SnapState`.
  Snapping is floating geometry, not a fill state: maximizing a snapped window
  (`set_fill_state_on`) or resizing it (`begin_resize`, keyboard resizes) calls
  `forget_snap`, and maximizing keeps the pre-snap rect as the restore geometry.
  `MoveSurfaceGrab` previews the edge snap target (`Pila::snap_preview`, a solid element
  drawn just above the windows) and applies it on release; dragging a snapped or filled
  window first restores its old size under the cursor (`begin_move`). Keyboard
  move/resize/center go through `apply_rect`, which also configures X11 windows.
- **Maximize/fullscreen** state lives in the toplevel's *pending* xdg state (`fill_state`
  reads it). `set_fill_state` is the single entry point; it stores the pre-fill geometry in
  the window's user data (`RestoreGeometry`) and restores it on exit; a window that
  started filled (no saved geometry) is restored to two thirds of its output, centered.
- **Decorations** (`decoration.rs`): pila prefers server-side and honors client requests
  (`handlers/xdg_shell.rs`); clients that don't bind xdg-decoration (GTK, weston) draw
  their own. A decorated window's `Space` location is still its geometry origin; the title
  bar (`TITLE_BAR_HEIGHT`) sits above it and an invisible resize border surrounds both.
  Hit testing goes through `Pila::under`, which for a window returns a `Hit` (client
  surface or `Decoration`) front to back; `surface_under` (pointer focus) ignores
  decorations. Layout (placement, maximize) uses the *pending* decoration mode via
  `layout_offset`; drawing and hit testing use the *committed* state (`has_title_bar`).
  Bar textures (buttons, title) are per-window `MemoryRenderBuffer`s cached by a key and
  re-rendered only when it changes; they can't be shared between windows (element ids must
  be unique per frame). Titles come from `text.rs` (cosmic-text): fonts load on a
  background thread at startup, and until then `render_title` returns `None` and bars draw
  without text.
- **Rounded corners** (`rounded.rs`, `corner-radius`, default 0): `Pila::scene` gives each
  window a `Rounding` (geometry plus title bar; none when maximized or fullscreen; radius
  at most half the shorter side and, with a bar, its height). `render::window_elements`
  then builds the window's elements itself (popups first and unclipped, as
  `Window::render_elements` does) and wraps its own surface elements that reach into a
  corner in `RoundedSurface`, whose `draw` overrides the default texture program with one
  that fades out the corner (`input_to_geo` inverts Smithay's `build_texture_mat`, so
  keep it in step with Smithay). Elements away from the corners stay plain (direct
  scanout). Content outside the geometry (CSD shadows) is not clipped. Opaque regions
  lose the corner squares. The bar background, bell flash and snap preview are
  `RoundedSolid` (pixel shader over a `SolidColorBuffer`'s element); the close button is
  rasterized with its corner cut. `ShapeDamage` (draws nothing, in front of the window)
  damages a window whose shape changed without a commit (e.g. a reload). Shaders are
  compiled once per EGL context; on failure corners stay square.
- **Resize** uses per-surface `ResizeSurfaceState` in the surface data map;
  `resize_grab::handle_commit` (called from `CompositorHandler::commit`) repositions the
  window when resizing from the top/left edges once the client commits the new size.

## Reference code

Smithay's `smallvil` (minimal stacking compositor) and `anvil` (full-featured) examples are
the canonical references. Clone `https://github.com/Smithay/smithay` outside `/workspace`
(e.g. `/tmp`) at the tag matching the locked `smithay` version in `Cargo.lock` (`v0.7.0`)
and read them before inventing new patterns.
