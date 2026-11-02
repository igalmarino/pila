# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# pila: a stacking Wayland compositor

A daily-driver **stacking** Wayland compositor (crate/binary name `pila`) written in **Rust**
on **Smithay 0.7.x**.

## Environment

- You run inside a dev container (Arch Linux base). The host is Arch Linux + GNOME (Wayland).
- The compositor is developed **nested**: Smithay's winit backend opens it as a window on
  the host GNOME desktop. `WAYLAND_DISPLAY` points at the host socket for that purpose;
  the compositor creates its own socket in `$XDG_RUNTIME_DIR` and clients launched from it
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

- Build: `cargo build`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt`
- Test: `cargo test`; single test: `cargo test <name_substring>`. Unit tests cover pure
  logic only (e.g. title bar/border hit math in `decoration.rs`); anything needing a
  `Window` or a client is tested nested.
- Run nested: `cargo run` opens a window on the host desktop and blocks. Don't leave it
  running in the foreground; use `timeout 10 cargo run` for smoke tests or ask the user.
  `cargo run -- -c '<cmd>'` runs `<cmd>` at startup instead of the terminal.
- Runtime env: `PILA_MOD` (alt default / super / ctrl), `PILA_TERMINAL` (default `foot`),
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
  (ext-data-control).
- Debugging: `WAYLAND_DEBUG=1` on a client for protocol traces, `RUST_LOG=debug` for the
  compositor.

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
loop's data is `Pila` itself (`EventLoop<'static, Pila>`, `Pila::loop_handle`), which Smithay
modules that schedule their own timers (e.g. idle-notify) require. Window-management logic lives in `state.rs`; the keybinding
list in `config.rs`'s module doc comment must be kept in sync with `action_for`, and the
`USAGE` string in `main.rs` with the CLI flags and `PILA_*` env vars.

Key flows and invariants:

- **Keybindings** are intercepted in the keyboard filter closure (`input.rs`); intercepted
  keycodes go in `suppressed_keys` so their release isn't forwarded to the client either.
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
- **Bare metal (udev):** one GPU (the seat's primary), outputs left to right, EDID off
  (make/model "Unknown", see `Cargo.toml`). pila draws named cursors from the xcursor
  theme (`cursor::CursorTheme`). Redraws are per output: `queue_redraw` schedules one
  idle `render_all`; an output with a frame in flight renders at its vblank
  (`frame_finish`), which also sends presentation feedback with the kernel's timestamp.
  The session lock is confirmed once every output has *shown* a locked frame.
  `PauseSession` (VT switch away) releases held keys. Libinput gets tap-to-click and
  keyboard LEDs; relative motion goes through the same constraint path as nested
  (`pointer_motion`) and is clamped to the outputs. Still missing: per-output scale,
  moving windows off removed outputs, `fullscreen_request`'s output, gestures, touch.
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
  must return is kept (`Pila::globals`). Fractional scale is always 1 nested; a backend
  with per-output scales must update surfaces as they move between outputs.
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
- **Maximize/fullscreen** state lives in the toplevel's *pending* xdg state (`fill_state`
  reads it). `set_fill_state` is the single entry point; it stores the pre-fill geometry in
  the window's user data (`RestoreGeometry`) and restores it on exit.
- **Decorations** (`decoration.rs`): pila prefers server-side and honors client requests
  (`handlers/xdg_shell.rs`); clients that don't bind xdg-decoration (GTK, weston) draw
  their own. A decorated window's `Space` location is still its geometry origin; the title
  bar (`TITLE_BAR_HEIGHT`) sits above it and an invisible resize border surrounds both.
  Hit testing goes through `Pila::under`, which for a window returns a `Hit` (client
  surface or `Decoration`) front to back; `surface_under` (pointer focus) ignores decorations. Layout
  (placement, maximize) uses the *pending* decoration mode via `layout_offset`; drawing and
  hit testing use the *committed* state (`has_title_bar`). Bar textures (buttons, title)
  are per-window `MemoryRenderBuffer`s cached by a key and re-rendered only when it changes;
  they can't be shared between windows (element ids must be unique per frame). Titles come
  from `text.rs` (cosmic-text): fonts load on a background thread at startup, and until
  then `render_title` returns `None` and bars draw without text.
- **Resize** uses per-surface `ResizeSurfaceState` in the surface data map;
  `resize_grab::handle_commit` (called from `CompositorHandler::commit`) repositions the
  window when resizing from the top/left edges once the client commits the new size.

## Reference code

Smithay's `smallvil` (minimal stacking compositor) and `anvil` (full-featured) examples are
the canonical references. Clone `https://github.com/Smithay/smithay` outside `/workspace`
(e.g. `/tmp`) at the tag matching the locked `smithay` version in `Cargo.lock` (`v0.7.0`)
and read them before inventing new patterns.
