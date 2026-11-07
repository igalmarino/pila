# pila's architecture

How pila is put together: its flows and invariants, a section per subsystem. Policies and
commands are in [CLAUDE.md](../CLAUDE.md), setup in [CONTRIBUTING.md](../CONTRIBUTING.md).
A change to a flow or invariant updates its section in the same commit.

Structure follows Smithay's `smallvil`. One state struct, `Pila` (`src/state.rs`),
implements every Smithay handler trait. Its fields are grouped by subsystem where they go together: Smithay's
protocol states (`Protocols`), Xwayland (`xwayland::XwaylandState`), screen capture
(`capture::Captures`), outputs beyond the `Space` (`output_management::Outputs`), windows
off the `Space` and their history (`Windows`), pila's own input state (`InputState`). Unlike smallvil (but like anvil), the calloop event
loop's data is `Pila` itself (`EventLoop<'static, Pila>`, `Pila::loop_handle`), which
Smithay modules that schedule their own timers (e.g. idle-notify) require. Core window
management lives in `state.rs`; each feature beyond it has its own module (named in the
flows below).

Key flows and invariants:

- **Configuration** (`config.rs`): a TOML file (`~/.config/pila/config.toml` or
  `--config`, documented in `config.example.toml`) parsed by `Config::from_toml` (pure,
  unit-tested; `PILA_*` env vars override it), loaded at startup by `Config::load`, which
  falls back to defaults on errors (`main` notifies of them once the session has had time to
  start a notification daemon, `state::notify_after_startup`), and reloaded when the file changes
  (`config_reload.rs`: inotify on its directory, a 250 ms settle timer, then
  `Config::read`; an invalid file keeps the running config and notifies). `apply_config`
  re-applies what changed: keyboard (`set_xkb_config`), rules (`refresh_rule_props`),
  `[input]` on connected devices
  (`UdevData::apply_input_config`), `[[output]]` (`Pila::apply_outputs` →
  `udev::apply_output_config`: enabled, mode via `use_mode`, scale, transform, position,
  adaptive sync; nested only scale) with wlr-output-management's runtime overrides on
  top (`Pila::outputs.overrides`; an edited section drops its monitor's). Anything that reads
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
  keyboard-shortcuts inhibitor (`shortcuts_inhibited`; all requests are granted, and sandboxed apps can't make any), or
  while the session is locked; Quit is the exception to the inhibitor (and, in debug
  builds, to the lock).
- **Backends** (`backend.rs`): `Pila::backend` is a `Backend` enum, `Winit` (nested,
  `winit.rs`), `Udev` (DRM/KMS + libinput + libseat on a TTY, `udev/`) or `Headless`
  (tests, `headless.rs`), chosen by `--backend` or by whether `WAYLAND_DISPLAY`/`DISPLAY`
  is set. `main` checks first that `libEGL.so.1` can be loaded (`missing_library`), since
  Smithay panics if it can't. Code outside the backends uses `Backend`'s methods (renderer for dmabuf
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
- **Monitors** (`edid.rs`, `Pila::outputs.monitors` by output name): on a connector connecting,
  `udev/` reads its `EDID` property and `edid::parse` takes make (the PNP code), model
  and serial from the base block; a missing or invalid one leaves "Unknown". They go to
  `wl_output` and `xdg-output` through `PhysicalProperties`, and to wlr-output-management
  in `output_heads`. An `[[output]]` section may name the monitor instead of the connector
  (`Config::output`: the monitor, then its model, then the connector); only that one
  section applies. `Pila::output_section` returns it under the output's name, since
  `Outputs::overrides` and `is_internal` go by the connector. Set before the output's settings
  are read, removed when it goes. Serials aren't logged. Headless outputs get a monitor from
  their spec.
- **Bare metal (udev):** `udev/mod.rs` has the session, the GPU and `UdevData`; `outputs.rs`
  connectors and outputs, `draw.rs` frames and vblanks, `input.rs` and `gamma.rs` the rest. one GPU (the seat's primary), outputs placed by their
  `[[output]]` config (mode, scale, position, transform) or else left to right. pila draws named cursors from the xcursor
  theme (`cursor::CursorTheme`; read by a thread at startup, the built-in arrow until
  then, so no frame reads a file). Redraws are per output: `queue_redraw` schedules one idle
  `render_all`; an output with a frame in flight renders at its vblank (`frame_finish`),
  which also sends presentation feedback with the kernel's timestamp. The session lock is
  confirmed once every output has *shown* a locked frame. `PauseSession` (VT switch away)
  releases held keys; `ActivateSession` scans the connectors and re-applies the outputs,
  since a monitor plugged in meanwhile can't come on without DRM master (`enable_output`
  waits). Libinput devices get keyboard LEDs and the
  `[input.touchpad]`/`[input.mouse]` settings (`configure_pointer`; a touchpad is a device
  that can tap) when added; relative motion goes through the same constraint path as
  nested (`pointer_motion`) and is clamped to the outputs. A removed output goes through
  `Pila::output_removed` (closes its layer surfaces, drops its lock surface, moves the
  pointer and stranded windows, incl. their restore geometry, via `fixup_windows`). Its
  `wl_output` global goes through `state::retire_global`: disabled now, removed 10 s later,
  since binding a removed global is a protocol error and a client may be doing that as it
  hears the news.
  With no output left on, `keep_a_screen_on` brings a built-in screen back (connector
  eDP/LVDS/DSI, `is_internal`) at the end of `scan_connectors` and in `connector_disconnected`
  before the gone output is unmapped, so its windows move there. There's
  no lid handling: a clamshell mode was tried and dropped, since logind suspends a
  closed laptop while no external output is on (e.g. between sessions).
  `fullscreen_request` honors the requested output for mapped windows
  (`set_fill_state_on`). Touchpad gestures (swipe, pinch, hold) are forwarded to the
  pointer focus through pointer-gestures (`gestures.rs`), except swipes with a finger
  count that `[gestures]` binds: those are kept whole from begin to end (`Pila::input.swipe`,
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
- **Window lifecycle:** `new_toplevel` pushes the window onto `Pila::windows.unmapped` (not the
  space). A client unmapping it (a null-buffer commit, `unmap_if_buffer_removed`) takes it
  out of the space, taskbars (`remove_foreign_toplevel`, which also stops its capture
  sessions), Mod+Tab and focus, and puts a new `Window` for the toplevel in `unmapped`:
  it maps again like a new window. `handle_xdg_commit` sends the initial configure, then on the first commit with a
  buffer moves it into `space` via `place_new_window` (centered on the pointer's output, or
  where its session restores it, see "Session management"; cascaded by `CASCADE_STEP` if
  the spot is taken). Lookups that must find unmapped windows
  use `window_for_surface`, which searches both.
- **Client rendering** (`handlers/compositor.rs`, `wait_for_client_rendering`): a
  pre-commit hook on every surface blocks a commit whose new dmabuf the client's GPU is
  still writing (`Dmabuf::generate_blocker`, polling its implicit fence). The commit applies
  when the fence signals (`blocker_cleared`, then `queue_redraw`), so a slow client delays
  its own frame and never pila's. Commits of one surface still apply in order. With
  linux-drm-syncobj (explicit sync; offered where the backend has a DRM device whose kernel
  has `syncobj_eventfd`: bare metal and headless with a GPU, not nested) a commit's acquire
  point takes the place of the fence, and Smithay signals the release point when the buffer
  is dropped. pila keeps no buffer of its own: animation snapshots are textures.
- **Content update pacing** (`commit_pacing.rs`, wp-fifo): Smithay blocks a commit waiting
  on a barrier; pila signals barriers. A commit that sets one registers its surface
  (`commit_paced`), `post_render` signals those shown on the output just drawn, and
  `after_frames` applies what that unblocked and asks for a redraw. So a client gets at most
  one content update per refresh. A barrier a frame didn't signal in a second is signaled by a
  timer (`signal_old_barriers`) wherever the surface is, so hidden clients and clients on an
  output turned off carry on, once a second. No damage means no flip, so a commit that changes
  nothing frees its barrier without waiting for a refresh, as with frame callbacks.
  wp-commit-timing: `hold_commit_until_timestamp` (pre-commit hook; Smithay's unmanaged mode)
  blocks a commit with a timestamp and arms one timer for the earliest deadline, a period
  before it. The redraw that wakes builds a frame, and before building it
  `signal_commit_timers` applies what will be due when it shows (`Backend::next_frame_time`:
  the next vblank from the kernel's timestamps, nested and headless a period from now).
  At the deadline itself anything left applies whether a frame came or not.
- **Stacking order** is the `Space` element order (back to front); raising = `raise_element`.
  `focus_window` both raises (with its family, see "Transient windows") and sets keyboard
  focus, then flushes pending configures.
- **Transient windows** (`transients.rs`): a window set on another (`xdg_toplevel.set_parent`,
  xdg-foreign, X11's `WM_TRANSIENT_FOR`) forms a family with it. `focus_window` raises the
  whole family (`raise_family`), each window before its children, so a child is always above
  its parent, even with the parent focused. `Space::map_element` raises the window it moves,
  so whatever moves a mapped window calls `keep_transients_above` after it. Minimizing hides
  the descendants too, marked to come back with the parent (`show_window`); one minimized
  alone stays minimized. `move_focused_to_workspace` parks the family, a child opens on its
  parent's workspace unless a rule says otherwise. A drag carrying a window to another
  workspace carries only that window. X11 can make cycles, so walks don't assume a tree.
  A modal child on screen (xdg-dialog; not X11: Smithay's window manager ignores
  `_NET_WM_STATE_MODAL`) takes the focus whenever its parent would get it: `focus_window` starts with `focus_redirect`, so clicks, Mod+Tab,
  activation and taskbars all agree. The pointer still reaches the parent: the client blocks
  its own widgets.
- **Window switching** (`switcher.rs`): `focus_window` moves the window to the front of
  `Pila::windows.focus_history` (most recent first) unless a Mod+Tab is in progress (`Pila::windows.switch`,
  a snapshot of that order). `cycle-focus`/`cycle-focus-back` step through the snapshot;
  the switch ends (`end_switch`, which records the choice) when Ctrl/Alt/Super are all
  released (checked after every key in `on_keyboard`), on a click, and in
  `release_all_keys`. Unmapped windows are dropped with `forget_focus_history`.
- **Minimize** (`minimize.rs`): a minimized window is unmapped from the `Space` and kept
  in `Pila::windows.minimized` with its location (X11 ones get `_NET_WM_STATE_HIDDEN`).
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
  Background; a fullscreen window on top of an output's windows also covers that output's Top
  (`top_layer_covered`, `upper_layers`, `lower_layers`), unless a
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
  WM_CLASS class), title and tag (xdg-toplevel-tag, empty if it set none), layer surfaces by namespace, with `*` wildcards
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
  foreign-toplevel identifier). Sessions live in `Pila::capture.sessions`; frames are rendered
  from `scene()` (window sources: only the window, no title bar) straight into the
  client's dma-buf (device and modifiers from `Pila::capture.dmabuf`, set by the backend),
  or offscreen and read back into shm (GL reads rows bottom-up, so they're reversed). A
  session's first frame is copied at once (idle), later ones in `after_frames` only when
  the session's own damage tracker sees a change. Size changes re-send constraints and
  fail the frame; a gone source stops the session (`stop_captures_of`). While locked,
  window captures wait. Cursor sessions are created stopped; `paint_cursors` is honored.
  `copy_frame` doesn't wait for the GPU on the event loop: with a native fence to export
  (bare metal, nested; not the headless backend) it returns `Copied::InFlight`, and
  `Pila::when_copied` finishes it (shm: maps the read-back) and sends `ready` from a
  calloop source on the fence fd. Without one it waits, as before.
- **Screencopy** (`screencopy.rs`, wlr-screencopy v3, for clients that predate
  ext-image-copy-capture): a frame names an output and optionally a region (logical,
  output-local); pila answers `buffer` (shm Xrgb8888), `linux_dmabuf` when captures may
  use dma-bufs, `buffer_done`. `copy` is rendered from the scene with `capture::copy_frame`
  after the next frame (it queues a redraw), so at most once per refresh; `copy_with_damage` waits in `Pila::capture.screencopies` until that
  manager's damage tracker for the output/region (`Captures::screencopy_trackers`) sees a change,
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
  in `Pila::outputs.overrides`, which `udev::effective_output_config` layers over the file
  for `connector_connected` and `apply_output_config`. If the backend can't do it
  (`apply_outputs` returns false: a failed modeset) the overrides go back to what they were
  and the client gets `failed`. Disabling a head
  (`disable_output`) clears its CRTC, removes its global and runs `output_removed`; the
  `OutputSurface` stays so the head is still listed; `enable_output` is the reverse (also
  used by `connector_connected`). Adaptive sync goes through `DrmCompositor::use_vrr`.
- **Blur** (`blur.rs`, ext-background-effect-v1): a surface's blur region is `Cacheable`
  surface state (`BlurRegion`, committed by Smithay); `Pila::scene` evaluates it per
  layer surface into `SceneLayer::blur` (upper layers only for now) and per window into
  `SceneWindow::blur` (`blur = true` in a rule covers the geometry and title bar, not a
  client's shadow; a window opaque over its geometry and at full opacity gets none,
  `blur::is_opaque`). Before an output's elements are built, the backends (and captures)
  call `BlurCache::prepare`: back to front, windows and then upper layers, so that each
  backdrop has those behind it, it renders what's behind the region (`render::elements_behind`)
  into an offscreen texture through a per-region damage tracker, and only when that
  changed runs the dual-Kawase passes (`BLUR_PASSES` levels, custom texture shaders,
  `Transform::Normal` throughout, textures with linear filtering and clamped edges). All
  of it covers the region's bounding box; what's outside the region (clients tessellate
  rounded shapes into strips) is then cleared to transparent (`Entry::mask`, worked out by
  edges so strips leave no gaps at a fractional scale, only when the region changes), and a
  new shape redoes it even with nothing changed behind. Damage only counts within
  `BLUR_REACH` of the region. Only a backdrop with no mask is opaque. A change of scale
  allocates anew, since a tracker keeps its scale. Entries are per surface and output: an
  output's `prepare` only drops its own, and `forget_output` those of one that's gone or
  turned off. `BlurCache::element` gives `render::scene_elements` the result (a `Texture` variant of
  `PilaRenderElement`), found by surface and output, to draw right behind that surface. A
  window's rounded corners are cut out by the last pass (`Shaders::up_rounded`, with the
  shape relative to the texture, so an output cutting it off doesn't move them). A window's
  backdrop is part of what `single_window_elements` draws: it goes into the texture
  of an animation or a slide, and the picture of a window closing or sliding away, fading
  with them but not with `opacity`. It's prepared with the windows where a slide will
  leave them. It's never declared opaque: what it hid would count as not shown (`frame.rs`)
  and its client drop to a frame a second while the blur still shows it. A window under
  an opaque one (`SceneWindow::covers`) isn't prepared, but keeps its entry. Shaders are
  compiled by `render::warm_up`, kept in the EGL context; a failure disables blur.
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
- **Activation** (`handlers/xdg_activation.rs`, xdg-activation): a token is issued only
  to the client holding keyboard focus, for a serial newer than that focus's enter. Tokens
  are single-use and expire after `TOKEN_TIMEOUT`. A request switches to the window's
  workspace (`reveal_workspace_of`), shows it if minimized and focuses it if it's mapped.
- **Session management** (`session_management.rs`, xdg-session-management-v1, on bindings
  in `session_protocol.rs` until a wayland-protocols release has them): a client keeps its
  toplevels in a session (a random id it stores) and asks to restore them. For windows in
  a session in use (`SessionManagement::tracked`), `refresh_sessions` (every `after_frames`,
  before its early return) keeps their floating rect (before a snap or fill), maximized and
  fullscreen. `restore_toplevel` of a known name applies them at the initial commit
  (`begin_session_restore`, before window rules, which win) and in `place_new_window`
  (`take_session_restore`: inside the usable area, `fit_in_area`; output gone: as usual).
  Until placed, `session_output` is the window's output for its scale and fill size. Saved
  to `$XDG_STATE_HOME/pila/sessions.json` by a thread (a second after a change, and at exit,
  `finish_sessions`), at most `MAX_SESSIONS`, empty ones going first; nested, and in a second
  pila on the same state (`take_sessions_lock`, a `scripts/tty` next to the login session),
  only in memory (`load_and_persist`). What a client can leave there is bounded
  (`MAX_TOPLEVELS`, `MAX_NAME_BYTES`, `MAX_CLIENT_SESSIONS`).
  Not `is_privileged`: it only touches the client's own windows. X11 windows and clients
  that don't use it open centered.
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
  pointer (`Pila::input.host_pointer`); a DRM backend sends real relative motion through the same
  checks.
- **X11** (`xwayland.rs`, `focus.rs`): pila starts Xwayland at launch and runs Smithay's
  `X11Wm`; spawned clients get its `DISPLAY` (never the host's) and the startup command
  waits for X (`pending_startup`). If it dies (`XwmHandler::disconnected`, `xwayland_gone`) its
  windows go, nothing else gets its `DISPLAY` and it's started again, at most `MAX_RESTARTS`
  times in `RESTART_WINDOW`. X11 windows are `Window::new_x11_window` in the same
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
  `Side` also names the four quarters (`snap-top-left`, ... or a drag near a corner:
  `edge_target`); a quarter is tiled only on the edges it touches.
- **Always on top, all workspaces** (`stacking.rs`): two marks in a window's user data, from a
  rule when it opens or a binding, shared by its transient family (`mark_new_window`,
  `inherit_marks`). `keep_on_top` puts the marked ones back above the others in their order, doing
  nothing if they already are (a change of order means damage). **Whatever raises a window has
  to end in it:** `raise_family` and `keep_transients_above` do, and so every
  `map_element(.., true)` is followed by one of them. `top_layer_covered` skips a marked window
  that isn't fullscreen itself, so one kept over a fullscreen window doesn't bring panels back.
  A window on all workspaces is never parked: `show_workspace` leaves it in the `Space`, raises
  it after focusing, and a slide draws it fixed (`SceneSlide::carried`); `workspace_of` says the
  active one; `move-to-workspace` removes the mark. Neither is saved with a session.
- **Between outputs** (`stacking.rs`): `move_focused_to_output` keeps the window's place relative
  to the usable area (`fit_in_area`), fills the new output if it was filled, or takes the same
  side (`move_snapped_to`), and translates where it goes back to. `focus_output` moves the pointer
  to the output's middle through `warp_pointer` (not if a client holds it) and focuses the top window
  there. Outputs go by position, around the end.
- **Show desktop** (`Pila::show_desktop`, `minimize.rs`): minimizes the workspace's windows without
  a transient parent and remembers them (`Windows::desktop_shown`); again, `show_window` in their
  order. `show_window` and `place_new_window` forget them, so the next press starts over.
- **IPC** (`ipc.rs`, `ipc_client.rs`; public format in `docs/IPC.md`): a Unix socket on the event
  loop, for `pila msg`, opened on every backend and removed on the way out. Nothing a client does may
  hold the loop: reads and writes don't block, a client gets `READS_PER_WAKE` reads per wake, what
  it hasn't read waits in `Client::unread` with a write source of its own, and past `MAX_UNREAD` or
  `MAX_REQUEST` it's disconnected. A client's place (`MAX_CLIENTS`) frees when its last source goes.
  Answers have their own JSON, not `state_json` (for tests, free to change), and are built only when
  asked. `action`, `focus` and `close` are refused while locked. An action goes through
  `Action::parse` and `run_action`, with a window switch ended at once, as no key is held.
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
- **Shadows** (`rounded.rs`, `[shadow]`, on by default): `Pila::scene_window` gives a window a
  `Shadow` (`SceneWindow::shadow`) from its frame and corners, stronger if it's focused. None
  for a maximized or fullscreen window or an override-redirect one, or for one whose `bbox` goes
  past its geometry (the client draws its own), unless a rule's `shadow` says otherwise.
  `ShadowElement` is a pixel shader from the distance to the frame: no texture and no pass of
  its own, and clear inside the frame, so it doesn't show through a translucent window or get
  into its blur. It comes last in `single_window_elements`, so it's behind the window's backdrop
  and in the backdrop of what's in front. `content_rect` includes it: textures of animations
  and snapshots hold it. Its id and commit are in the window's user data, and the commit only
  changes with its look (`ShadowState`): moving is the damage tracker's job, and a frame only
  draws it where something over or under it changed. Never opaque, and out of hit testing.
- **Colors** (`[colors]`, `config::Colors`, premultiplied): `Scene::colors` for the clear color,
  `SceneTitleBar::colors` for the bar. Rasterized buttons and titles are keyed by their colors,
  so a reload redraws them. The bar stays opaque, which `SceneWindow::covers` counts on.
- **Resize** uses per-surface `ResizeSurfaceState` in the surface data map;
  `resize_grab::handle_commit` (called from `CompositorHandler::commit`) repositions the
  window when resizing from the top/left edges once the client commits the new size.
