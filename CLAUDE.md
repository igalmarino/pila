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
  logic only (e.g. title bar/border hit math in `decoration.rs`, config parsing, including
  `example_config_is_valid` for `config.example.toml`); anything
  needing a `Window` or a client is tested nested.
- Run nested: `cargo run` opens a window on the host desktop and blocks. Don't leave it
  running in the foreground; use `timeout 10 cargo run` for smoke tests or ask the user.
  `cargo run -- -c '<cmd>'` runs `<cmd>` at startup instead of the terminal.
- Config: `config.example.toml` (copy to `~/.config/pila/config.toml`, or `--config`).
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
- Debugging: `WAYLAND_DEBUG=1` on a client for protocol traces, `RUST_LOG=debug` for the
  compositor.
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
  re-applies what changed: keyboard (`set_xkb_config`), `[input]` on connected devices
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
  `winit.rs`) or `Udev` (DRM/KMS + libinput + libseat on a TTY, `udev.rs`), chosen by
  `--backend` or by whether `WAYLAND_DISPLAY`/`DISPLAY` is set. Code outside the backends
  uses `Backend`'s methods (renderer for dmabuf imports, `queue_redraw`, `cursor_changed`,
  LEDs, VT switch) rather than matching. Both feed the same `scene()`, `render.rs`,
  `frame.rs` and `post_render`. `Pila::new` hard-codes a keyboard and pointer for both.
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
  `render.rs` (backend-neutral; `PilaRenderElement`), and calls the damage tracker's
  `render_output` directly. Smithay's `space::render_output` isn't used because it can't
  interleave title bars with windows. Afterwards `frame.rs` (backend-neutral) records each
  drawn surface's primary output from the render states, sends presentation-time feedback
  once the frame is submitted, and sends frame callbacks only to surfaces whose primary
  output it is (surfaces shown nowhere: once a second, `HIDDEN_FRAME_THROTTLE`, so a
  covered animating client doesn't spin); frame callback times and
  feedback both use `Pila::clock` (CLOCK_MONOTONIC). Once per event-loop
  iteration, however many outputs were drawn, `after_frames` (`main.rs`) runs captures,
  screencopies, `space.refresh()`, taskbar updates and `popups.cleanup()`, then a flush. Buffer age (for partial repaints) needs the window's EGL
  surface made current first (`winit::make_surface_current`); Smithay's `bind` is lazy,
  and querying without it fails with `EGL_BAD_SURFACE` every animated frame.
- **Redraws are on demand** (`Pila::queue_redraw`): after every client dispatch (display
  source), every winit event, and while a resize needs full frames or a lock awaits its
  first locked frame. Nothing else redraws, so state changes outside those paths (timers,
  `insert_idle` callbacks) must call `queue_redraw` themselves. Clients are flushed after
  every event-loop iteration (`main.rs`), not only on redraw.
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
  space). `handle_xdg_commit` sends the initial configure, then on the first commit with a
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
  Background; a fullscreen window on top also covers Top (`layer_order`). All pointer
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
  bus alone, so a test instance leaves the login session's services alone. Nested, apps
  use the host's portals.
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
- **Resize** uses per-surface `ResizeSurfaceState` in the surface data map;
  `resize_grab::handle_commit` (called from `CompositorHandler::commit`) repositions the
  window when resizing from the top/left edges once the client commits the new size.

## Reference code

Smithay's `smallvil` (minimal stacking compositor) and `anvil` (full-featured) examples are
the canonical references. Clone `https://github.com/Smithay/smithay` outside `/workspace`
(e.g. `/tmp`) at the tag matching the locked `smithay` version in `Cargo.lock` (`v0.7.0`)
and read them before inventing new patterns.
