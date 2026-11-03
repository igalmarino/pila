# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# pila: a stacking Wayland compositor

A daily-driver **stacking** Wayland compositor (crate/binary name `pila`) written in **Rust**
on **Smithay 0.7.x**.

## Environment

- You run inside a dev container (Arch Linux base). The host is Arch Linux + GNOME (Wayland).
- The compositor is developed **nested**: Smithay's winit backend opens it as a window on
  the host desktop. `WAYLAND_DISPLAY` points at the host socket for that purpose
  (`/run/host-wayland/wayland-0`, a hard link `scripts/link-wayland` makes to the socket of
  whichever session the container was entered from: GNOME, or pila itself on a TTY); the
  compositor creates its own socket in `$XDG_RUNTIME_DIR` and clients launched from it
  connect there.
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

- Build: `cargo build`; `cargo build --release` for what `scripts/tty` runs.
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt`
- Test: `cargo test`; single test: `cargo test <name_substring>`. Unit tests cover pure
  logic only (e.g. title bar/border hit math in `decoration.rs`, config parsing, including
  `shipped_configs_are_valid` for `config.example.toml` and `resources/daily`); anything
  needing a `Window` or a client is tested nested.
- Run nested: `cargo run` opens a window on the host desktop and blocks. Don't leave it
  running in the foreground; use `timeout 10 cargo run` for smoke tests or ask the user.
  `cargo run -- -c '<cmd>'` runs `<cmd>` at startup instead of the terminal.
- Config: `config.example.toml` (copy to `~/.config/pila/config.toml`, or `--config`).
  Env overrides: `PILA_MOD` (alt default / super / ctrl), `PILA_TERMINAL` (default `foot`),
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
- TTY runs (the user, on the host): `scripts/tty [config]` runs the release build
  (`--debug` for the debug one) under `dbus-run-session`; read `pila-tty.log` and the
  previous run's `pila-tty.log.old` in the repo root afterwards. The user's day-to-day
  config lives in `resources/daily` (installed by `scripts/install-config`); keep it
  valid (a test parses it, and `config.example.toml`).
- The user develops from inside pila on a TTY: this container is then connected to that
  pila, so restarting it ends the Claude session, and `cargo run` opens a nested window
  inside it. Changes that need a pila restart are tested by the user.
- Git: small commits straight to `main`, after the user has checked anything visual or
  hardware-related; push to GitHub (`origin`, public, GPL-3.0-or-later) only when asked.

## Language

Everything in the repository is in English: code, identifiers, comments, docs, log
messages, test names, config examples and commit messages, even when the conversation
isn't.

## Protocol policy

pila implements **standard protocols only**: Wayland core and the official
`wayland-protocols` (stable, staging, and unstable where nothing newer exists). Where both
exist, prefer the `ext-`/staging protocol over an older unstable or vendor one (e.g.
`ext-data-control`, not `zwlr_data_control`). Vendor protocols (`wlr-*`, KDE, GNOME,
…) are not added, even if Smithay implements them.

The single exception is **`zwlr_layer_shell_v1`**: there's no standard layer shell yet and
panels, launchers, wallpapers and notification daemons all need it. Replace it once an
`ext` layer shell lands in `wayland-protocols` and Smithay. Features with no standard
protocol (window management for taskbars, output configuration) wait for one, or go
through pila's own config/IPC. A standard protocol that Smithay doesn't implement (e.g.
`ext-image-copy-capture` for screen capture) is implemented in pila on the bindings from
the `wayland-protocols` crate, not swapped for a vendor one.

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
  (`UdevData::apply_input_config`), `[[output]]` (`udev::apply_output_config`: mode via
  `use_mode`, scale, position; nested only scale). Anything that reads `self.config` when
  used (bindings, Mod, terminal) needs nothing; `autostart` isn't re-run. A new setting
  must either be read when used or be re-applied there. Keep in sync: `DEFAULT_BINDINGS`,
  the binding table in `config.rs`'s module doc and the defaults listed in
  `config.example.toml`; and the `USAGE` string in `main.rs` with the CLI flags and env
  vars.

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
  `[[output]]` config (mode, scale, position) or else left to right, EDID off (make/model
  "Unknown", see `Cargo.toml`). pila draws named cursors from the xcursor theme
  (`cursor::CursorTheme`). Redraws are per output: `queue_redraw` schedules one idle
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
  pointer focus through pointer-gestures (`gestures.rs`); pila binds none itself. Still
  missing: output rotation, touch.
- **Render loop** (`winit::redraw`): collects `Pila::scene()` (windows front to back with
  their title bars) and the cursor *before* borrowing the backend, builds elements with
  `render.rs` (backend-neutral; `PilaRenderElement`), and calls the damage tracker's
  `render_output` directly. Smithay's `space::render_output` isn't used because it can't
  interleave title bars with windows. Afterwards `frame.rs` (backend-neutral) records each
  drawn surface's primary output from the render states, sends presentation-time feedback
  once the frame is submitted, and sends frame callbacks; frame callback times and
  feedback both use `Pila::clock` (CLOCK_MONOTONIC). Then `space.refresh()`,
  `popups.cleanup()` and a flush. Buffer age (for partial repaints) needs the window's EGL
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
  (`forget_minimized`). It comes back only through Mod+Tab: the switch order includes
  minimized windows; a minimized pick is shown (`show_window`) while selected, hidden
  again (`hide_window`) when the switch moves on, and stays when the switch ends.
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
  `preferred_buffer_scale` (wl_compositor v6) for their surface's primary output after
  every frame (`update_surface_scales` from `post_render`), and before their first
  configure (`send_initial_scale`). Xwayland stays at scale 1 (upscaled).
- **Screen capture** (`capture.rs`, ext-image-capture-source + ext-image-copy-capture, on
  the `wayland-protocols` bindings): sources are an output or a window (by its
  foreign-toplevel identifier). Sessions live in `Pila::captures`; frames are rendered
  from `scene()` (window sources: only the window, no title bar) straight into the
  client's dma-buf (device and modifiers from `Pila::capture_dmabuf`, set by the backend),
  or offscreen and read back into shm (GL reads rows bottom-up, so they're reversed). A
  session's first frame is copied at once (idle), later ones in `post_render` only when
  the session's own damage tracker sees a change. Size changes re-send constraints and
  fail the frame; a gone source stops the session (`stop_captures_of`). While locked,
  window captures wait. Cursor sessions are created stopped; `paint_cursors` is honored.
- **Session environment** (`Pila::spawn`, `start_session`): children get `WAYLAND_DISPLAY`,
  pila's Xwayland `DISPLAY`, `XDG_CURRENT_DESKTOP=pila` and `XDG_SESSION_TYPE=wayland`.
  Screen sharing goes through xdg-desktop-portal + xdg-desktop-portal-wlr (PipeWire),
  which `resources/xdg/xdg-desktop-portal/pila-portals.conf` selects; xdpw's own config
  (`resources/xdg/xdg-desktop-portal-wlr/pila`: pick the screen with slurp) must be in
  `~/.config/xdg-desktop-portal-wlr/` (xdpw ignores `XDG_CONFIG_DIRS`). On a TTY, before the
  startup programs, pila exports that environment to the D-Bus session bus
  (`export_environment`), but only to a bus of its own (`scripts/tty` uses
  `dbus-run-session`): the systemd user bus is shared with e.g. a GNOME session on another
  VT, whose portals it would break. Nested, apps use the host's portals.
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
  the window's user data (`RestoreGeometry`) and restores it on exit.
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
