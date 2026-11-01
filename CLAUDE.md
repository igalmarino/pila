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
- The DRM/KMS + libinput backend cannot be tested in this container (no seat, no TTY).
  That needs the host, from a TTY. Write that code, but ask the user to test it.
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
  while nested.
- Test clients available inside the nested compositor: `foot`, `weston-terminal`,
  `weston-simple-egl`, `wayland-info`, and `gtk4-widget-factory` / `gtk4-demo` (menus,
  popovers, dropdowns: the way to exercise popups and popup grabs). `foot` and GTK use
  cursor-shape (named cursors); `weston-terminal` draws its own cursor surfaces.
- Debugging: `WAYLAND_DEBUG=1` on a client for protocol traces, `RUST_LOG=debug` for the
  compositor.

## Architecture

Structure follows Smithay's `smallvil`. One state struct, `Pila` (`src/state.rs`),
implements every Smithay handler trait; the calloop event loop carries `CalloopData
{ state, display_handle }`. Window-management logic lives in `state.rs`; the keybinding
list in `config.rs`'s module doc comment must be kept in sync with `action_for`, and the
`USAGE` string in `main.rs` with the CLI flags and `PILA_*` env vars.

Key flows and invariants:

- **Keybindings** are intercepted in the keyboard filter closure (`input.rs`); intercepted
  keycodes go in `suppressed_keys` so their release isn't forwarded to the client either.
- **Nested-only assumptions:** `cursor_image` in `handlers/mod.rs` stores the request in
  `Pila::cursor_status` and forwards it to the winit window (`winit::apply_cursor`): named
  shapes are drawn by the host; for client cursor surfaces the host cursor is hidden and
  pila renders the surface itself (`src/cursor.rs`, backend-neutral; the winit redraw
  passes it to `render_output` as a custom element and sends it frame callbacks). Winit
  doesn't report the pointer leaving the window, so a drawn cursor lingers at the edge.
  `Pila::new` hard-codes a keyboard and pointer. A DRM/libinput backend must also draw
  named shapes itself (xcursor theme) and track input-device hotplug. `Pila::backend` holds the winit backend so `DmabufHandler`
  (`handlers/compositor.rs`) can test-import client buffers; a DRM backend needs its own
  renderer there and must create the linux-dmabuf global itself (see `winit::init_dmabuf`).
  Other single-output, winit-only shortcuts: `process_input_event` handles only absolute
  pointer motion (libinput sends relative motion), motion is mapped onto the first output,
  and `fullscreen_request` ignores the requested output. Smithay is built with
  `default-features = false` and only the winit features, so a DRM backend must also add
  features such as `backend_drm`, `backend_libinput` and `backend_session_libseat` in `Cargo.toml`.
- **Render loop** (`winit::redraw`): collects `Pila::scene()` (windows front to back with
  their title bars) and the cursor *before* borrowing the backend, builds elements with
  `render.rs` (backend-neutral; `PilaRenderElement`), and calls the damage tracker's
  `render_output` directly. Smithay's `space::render_output` isn't used because it can't
  interleave title bars with windows. Then frame callbacks, `space.refresh()`,
  `popups.cleanup()`, flush, and the next redraw (it redraws continuously).
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
- **Maximize/fullscreen** state lives in the toplevel's *pending* xdg state (`fill_state`
  reads it). `set_fill_state` is the single entry point; it stores the pre-fill geometry in
  the window's user data (`RestoreGeometry`) and restores it on exit.
- **Decorations** (`decoration.rs`): pila prefers server-side and honors client requests
  (`handlers/xdg_shell.rs`); clients that don't bind xdg-decoration (GTK, weston) draw
  their own. A decorated window's `Space` location is still its geometry origin; the title
  bar (`TITLE_BAR_HEIGHT`) sits above it and an invisible resize border surrounds both.
  Hit testing goes through `Pila::element_under`, which returns a `Hit` (client surface or
  `Decoration`) front to back; `surface_under` (pointer focus) ignores decorations. Layout
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
