# Pila's architecture

How Pila is put together: its flows and invariants, a section per subsystem. Policies and
commands are in [CLAUDE.md](../CLAUDE.md), setup in [CONTRIBUTING.md](../CONTRIBUTING.md).
A change to a flow or invariant updates its section in the same commit.

The crate is a library (`src/lib.rs`: every module, `Pila`, `version()`) and the `pila`
command (`src/main.rs`: arguments, `USAGE`, `--check-config`, the panic hook, `main`); the
library is what the fuzz targets (`fuzz/`, cargo-fuzz, a workspace of its own on nightly; see
CONTRIBUTING.md) call: `ipc::parse_request`, `Config::from_toml`, `edid::parse`,
`gamma_control::read_ramp`, and a headless Pila for the protocol target.

Structure follows Smithay's `smallvil`. One state struct, `Pila` (`src/state.rs`),
implements every Smithay handler trait. Its fields are grouped by subsystem where they go together: Smithay's
protocol states (`Protocols`), Xwayland (`xwayland::XwaylandState`), screen capture
(`capture::Captures`), outputs beyond the `Space` (`output_management::Outputs`), windows
off the `Space` and their history (`Windows`), Pila's own input state (`InputState`). Unlike smallvil (but like anvil), the calloop event
loop's data is `Pila` itself (`EventLoop<'static, Pila>`, `Pila::loop_handle`), which
Smithay modules that schedule their own timers (e.g. idle-notify) require. Core window
management lives in `state.rs`; each feature beyond it has its own module (named in the
flows below).

Key flows and invariants:

- **Configuration** (`config.rs`): a TOML file (`~/.config/pila/config.toml` or
  `--config`, documented in `config.example.toml`, which `main` writes to the default path
  when nothing is there, without `--config`: `config::create_default_file`, before the event
  loop; never over a file or a symlink, and a failure only logged; then, with `tool-configs`,
  `tool_configs::prepare_for_session` writes waybar's, fuzzel's, mako's and swaylock's
  configurations from `resources/tools/` into `$XDG_RUNTIME_DIR/pila-tools` (`Pila::tool_configs_dir`;
  each file renamed into place from a temporary file of its own, `partial_path`, so a nested Pila
  or a tool starting never reads one half written), their `@key@`s filled with `[colors]`
  (`tool_configs::fill`; again, on a thread, when a reload changes them, one rewrite at a time:
  `tool_configs::rewrite_in_background`, for each tool's next start),
  text at `text::FONT_SIZE` px in sans-serif, as the title bars, waybar's with
  Font Awesome symbols where a font file under fontconfig's default directories has "awesome"
  in its path, `icon_font_installed`: nothing in the user's directories. With them goes
  swaybg's wallpaper, which Pila draws (`wallpaper.rs`: `background` with a small mark in the
  bottom-right corner, outlined rectangles piled like windows in greys between `background` and
  `hover` and the accent, on a 1920x1080 grid drawn at 4K, a PNG of rows filtered `Up`, ~11 ms
  in a release build, so it's paid at startup and on the rewrite's thread); swaybg reads no file
  of the user's, so a bare `swaybg` always gets it, `-i ... -m fit -c <background>` (a tool's
  arguments name `[colors]` keys as `@key@`, `fill`): the whole picture on any monitor's shape,
  where `fill` would crop the corner's mark. `Pila::spawn` gives
  them to the example's own commands as written (`config::is_example_command`: its `launcher`
  and `autostart`), on their command line (`tool_configs::with_configs`: the tool's arguments
  after each word that is its name, swaylock inside the swayidle line too), for each tool with
  no configuration of the user's (`tool_configs::user_has_config`, at every spawn, so one
  copied into place takes over); a reload that turns `tool-configs` on writes them at the
  first such spawn) parsed by
  `Config::from_toml` (pure,
  unit-tested; `PILA_*` env vars override it), loaded at startup by `Config::load`, which
  falls back to defaults on errors (`main` notifies of them once the session has had time to
  start a notification daemon, `state::notify_after_startup`), and reloaded when the file changes
  (`config_reload.rs`: inotify on its directory, a 250 ms settle timer, then
  `Config::read`; an invalid file keeps the running config and notifies). Notifications
  (`send_notification`, in the log too) go to the notification daemon over D-Bus
  (`notifications.rs`: `Notifier::send` hands them to a thread of their own, started with the
  first, which calls `org.freedesktop.Notifications.Notify` on the session bus Pila's programs
  get, `[environment]`'s `DBUS_SESSION_BUS_ADDRESS` else Pila's, and keeps the connection); with
  no bus or no daemon the log says so once, and the next one tries again. `apply_config`
  re-applies what changed: keyboard (`set_xkb_config`; a new keymap starts with every lock
  off, so `numlock` turns Num Lock on again, as at startup: `state::lock_num_lock`), rules (`refresh_rule_props`),
  the workspace count (`apply_workspace_count`), `[input]` on connected devices
  (`UdevData::apply_input_config`), `[[output]]` (`Pila::apply_outputs` →
  `backend::apply_output_config`: on udev and headless enabled, mode via `use_mode`, scale,
  transform, position, adaptive sync; nested only scale; then `fit_to_outputs`) with
  wlr-output-management's runtime overrides on top (`Pila::outputs.overrides`; an edited section drops its monitor's). Anything that reads
  `self.config` when used (bindings, Mod, terminal) needs nothing; `autostart` isn't
  re-run. A new setting must either be read when used or be re-applied there. A valid file
  with a mistake Pila can tell (an `[[output]]` named twice: `Config::output` takes the first)
  carries it in `Config::warnings`: `--check-config` prints it, startup and a reload notify it
  (a reload "with a warning", for as long as an error). The defaults
  are `config.example.toml`'s active values (`config::EXAMPLE`, built in) and nowhere in the
  code: `Config::parse_with` parses the user's file and the example into the same all-optional
  structs (`ConfigFile`), so errors point at the user's lines, and fills each setting the user's
  leaves out from the example's (`or_example!`; an animation's `true` is the example's
  duration); one neither sets is an error (`config::need`), which
  `every_default_comes_from_the_example` checks happens for each one. Only what depends on the
  machine is worked out when unset (`mod`, `terminal`, the keymap; a device's libinput options
  are libinput's). The bindings and gestures are the example's `[bindings]` and `[gestures]`,
  then the workspace digits, then the user's (a later one for the same combo replaces an
  earlier one); `default-bindings = false` leaves out all but the user's. Keep in
  sync: the actions, `NAMED_ACTIONS` and `ACTIONS_WITH_ARGUMENT`, and `config.example.toml`'s
  list of them (checked); the `USAGE` string in `main.rs` with the
  CLI flags and env vars; and the user docs, `docs/MANUAL.md` and the man page `docs/pila.1`
  (tests check both against the default bindings and the man page against `USAGE`).
- **Keybindings** are intercepted in the keyboard filter closure (`input.rs`), which looks
  up every key press in `Config::bindings` (modifiers must match exactly, Caps/Num Lock
  ignored; keys without Mod, e.g. media keys, can be bound); intercepted keycodes go in
  `suppressed_keys` so their release isn't forwarded to the client either. A binding with
  `repeat = true` (`config::Binding::repeat`, `config::BindingFile` parses the table form) runs
  again while its key is held: once it has run, `start_binding_repeat` inserts a calloop timer
  at `repeat-delay`, then every 1/`repeat-rate` s (the keyboard sends nothing while a key is
  held: repeat is the receiver's), which checks the lock, window menu and inhibitor again; its
  key's release, another key's press and `release_all_keys` end it (`stop_binding_repeat`; the
  timer ends itself if its own action stopped it, `InputState::repeat_firing`). A modifier
  alone (`"Mod" = "launcher"`, a default) is a tap: a modifier key's press is never looked
  up (`action_for` skips the keysyms `tap_sym` knows) but arms `InputState::mod_tap`, which
  any other key press, pointer button, scroll, gesture or touch, or `release_all_keys`
  (nested focus loss), clears; its release, if still armed, looks up `tap_action` (same
  lock and inhibitor filters) and runs it after the release was forwarded to the client,
  which saw the press like any modifier's. Scrolling
  (`on_pointer_axis`) goes to the surface under the pointer: a wheel step as `value120` 120
  plus 15 px, a touchpad's pixels, with a stop when the fingers lift. Before that, a mouse
  wheel's steps with the held modifiers may be a binding (`Trigger::Wheel`,
  `wheel_bindings`): each notch (`value120` summed in `InputState::wheel_v120`, for
  high-resolution wheels; turning back starts over) runs it once, except within
  `wheel-binding-cooldown` of the last run on that axis (`InputState::wheel_ran`, wall
  clock: event times differ by backend), where it's dropped, so a quick turn of the wheel
  is one workspace and not all of them; and a bound axis isn't
  sent to the client; the keyboard's filters apply (locked, window menu open, shortcuts
  inhibited: the scroll is the client's). The filter runs before an input method's keyboard
  grab, so the input method never sees a key Pila used (see "Input methods").
  Bindings don't run while the focused surface (or its menu's window) holds an active
  keyboard-shortcuts inhibitor (`shortcuts_inhibited`; all requests are granted, and sandboxed apps can't make any), or
  is an X11 window whose app grabbed the keyboard (XGrabKeyboard, which Xwayland turns into
  xwayland-keyboard-grab, a global Smithay shows Xwayland alone: `xwayland::has_keyboard_grab`;
  not Smithay's keyboard grab, which would hold the focus where it is and take the device's one
  grab from an input method), or
  while the session is locked; Quit is the exception to the inhibitor (and, in debug
  builds, to the lock). Before either check, a key that wakes powered-off monitors does only
  that, Ctrl+Alt+F<N> (the keymap's `XF86_Switch_VT_<N>`) switches VT (`Action::ChangeVt`,
  which `run_action` also runs while locked), and an open window menu takes every press.
- **Virtual keyboards** (`virtual_keyboard.rs`, zwp_virtual_keyboard_v1): Smithay hands
  their events in like a device's, and they are keyboards: keys go through
  `process_input_event` (bindings, `suppressed_keys`, idle activity) with the device's own
  `KeyboardSource` (`on_keyboard`), so a virtual release never releases a key the user holds.
  A device's keymap replaces the seat's while it types (`activate_virtual_keymap`;
  `InputState::virtual_keymap` says whose); the next physical key or the device going away
  brings the configured one back (`restore_configured_keymap`) from its saved text
  (`InputState::configured_keymap`, no recompile), with the layout and Caps/Num Lock from
  before. A device with no keymap, or the configured one, doesn't swap. `apply_keyboard_config`
  drops both. The input method's own (the client holding the keyboard grab,
  `InputMethodHandle::keyboard_grab_client`) is the exception: what it types are keys it
  hands back, and they go straight to the focus (`forward_input_method_key`; see "Input
  methods").
- **Input methods** (`input_method.rs`, text-input-v3 and input-method-v2, both Smithay's):
  text-input is for every client, input-method only for privileged ones (it reads everything
  typed). Smithay follows the keyboard focus: `KeyboardTarget` of a `WlSurface` (which
  `focus::KeyboardFocus` delegates to) moves the text input's focus with it, sends `enter`/
  `leave` while there's an input method, and deactivates it on leaving; an app's `enable`
  activates it. The input method usually grabs the keyboard while active
  (`InputMethodKeyboardGrab`, a `KeyboardGrab`): `on_keyboard` runs Pila's filter (bindings,
  `suppressed_keys`) first, so the input method never sees Pila's keys, then forwards through
  the grab. What it doesn't use it hands back through a virtual keyboard of its own: those keys
  go to the focus past the grab (`input_forward_bypassing_grab`), with no filter and not
  through Smithay's tracking of held keys, which would drop a key the physical keyboard still
  holds (through the grab they would loop back to it). Its candidate window (an
  `input_popup_surface`) is tracked by the `PopupManager` as `PopupKind::InputMethod`, so
  it's drawn, hit and redrawn with its window's or layer surface's popups. Its parent is the
  surface with the keyboard: a window's, a layer surface's, or a popup of either (a popover
  with a text field), in whose root's tree it's kept (and dismissed from: `dismiss_popup`
  looks the root up). Its location is in the parent surface's coordinates, as the text
  cursor's: `parent_geometry` gives the parent's own geometry (a window's, a popup's; none
  for a layer surface), which the drawing takes off (`render.rs` draws popups from their
  root's geometry). `place_popup` puts it below the text cursor (one with no width, as GTK
  sends, is kept by Pila's fork of Smithay; Smithay's own drops it), above if it doesn't fit,
  slid sideways to stay in its room (`popup_bounds`): the output the text cursor is on (a
  window may span two), without the panels drawn above windows, which would cover it
  (`output_area`, usable, unless a fullscreen window covers the top layer:
  `top_layer_covered`); all of the output for a layer surface's. It's placed
  (`place_input_method_popup`) on creation, when the app moves its text cursor, and on each of
  its commits, as its size changes with what's typed. Smithay moves it onto the text cursor
  before telling Pila, so where Pila last put it is kept in its surface's data (`PlacedAt`,
  cleared when it's dismissed: Smithay shows it again without a parent to dismiss first);
  moving it from there is a change of Pila's own, redrawn at once (`Content::Sure`) where it
  was and where it goes. A new one gets its output's scale before its first buffer, as a
  window does. With no text field (the input method deactivated: Smithay takes its parent
  away) it shows nowhere, whatever its client draws (`surface_outputs`). Grabs: see "Pointer
  grabs". The lock screen never gets the input method: while locked, keys go to the focus
  past its grab (`Pila::forward_key`, which `on_keyboard` and `release_all_keys` forward
  with), and a lock surface given the keyboard loses the text input at once
  (`KeyboardTarget::enter` of `KeyboardFocus`), so the input method neither activates there
  nor sees what's typed.
- **Drag and drop:** a client's `start_drag` reaches `dnd_requested`
  (`handlers/mod.rs`), which runs Smithay's `DnDGrab` on the pointer or touch point that
  started it; Smithay's default would cancel every drag. Its icon (`Pila::dnd_icon`) is
  `Scene::dnd_icon`, drawn by `output_elements` at the pointer plus its offset (moved by the
  icon's `buffer_delta`, `handle_dnd_icon_commit`) above everything but the cursor, given frame
  callbacks with the cursor (`frame::other_trees`), redrawn as the pointer moves, and dropped
  by `DndGrabHandler::dropped`/`cancelled`. Smithay never sends the source
  `wl_data_source.target`.
- **Backends** (`backend.rs`): `Pila::backend` is a `Backend` enum, `Winit` (nested,
  `winit.rs`), `Udev` (DRM/KMS + libinput + libseat on a TTY, `udev/`) or `Headless`
  (tests, `headless.rs`), chosen by `--backend` or by whether `WAYLAND_DISPLAY`/`DISPLAY`
  is set. `main` checks first that `libEGL.so.1` can be loaded (`missing_library`), since
  Smithay panics if it can't. Code outside the backends uses `Backend`'s methods (renderer for dmabuf
  imports, `queue_redraw`/`queue_redraw_output`, `draws_cursor`, `cursor_changed`, LEDs, VT switch) rather than matching. All
  feed the same `scene()`, `render.rs`, `frame.rs` and `post_render`. `Pila::new`
  hard-codes a keyboard and pointer for all. Each renderer, when created, goes through
  `render::warm_up` (the rounded-corner and blur shaders, and a draw into an offscreen texture of
  what a window is made of: a solid, a rounded one, a texture uploaded from memory, blended),
  so the first frame that needs them doesn't pay for it: a driver builds what a kind of draw needs
  when it's first made, not when its shader is compiled, which more than doubled the first frame
  of the first window opened.
- **Panics** (`sync.rs`, `main::log_panics`): past startup a panic ends the session, so the code has no
  `unwrap`/`expect` that rests on more than a local, stated invariant: mutexes go through
  `sync::Locked::locked` (a poisoned one only follows a panic that already ended Pila), user
  data through `get_or_insert`, the rest through `let`-`else` (with a log where it means
  something went wrong). A panic, on any backend, is logged as "pila
  crashed" with its location, the version and a backtrace, captured whatever `RUST_BACKTRACE`
  says (`main::log_panics`): a login session doesn't set it, and a tester's first report should
  say where it happened.
- **Headless (tests):** virtual outputs (`--headless-outputs NAME:WxH[@HZ],...`; each
  also offers 1024x768 and 1920x1080) rendered into offscreen textures on a render node
  or Mesa's software device, each paced by a timer at its refresh rate (at most one redraw
  per period, `HeadlessOutput::last_render`) while a redraw is asked for it; `[[output]]` applies in full (`headless::apply_output_config`, with
  wlr-output-management's overrides), gamma, power and adaptive sync (every virtual output
  can do it, reported back on its head as udev does) are kept in memory. Mod defaults
  to Super and nothing starts without `-c`/autostart. The control socket
  (`test_control.rs`, `$XDG_RUNTIME_DIR/<WAYLAND_DISPLAY>.control`, JSON lines; its
  commands are in the module doc) injects `TestInput` events through
  `process_input_event` (pointer moves are relative motion), reads pixels of the last
  frame, adds/removes outputs, and answers `state` with `Pila::state_json`
  (`introspect.rs`: windows with geometry, stacking, workspace, fill/snap state,
  decorations and rule props; focus, Mod+Tab, layers, outputs, pointer, lock,
  animations). A new feature exposes there what its tests need to check. With `PILA_HEADLESS_CHECK_REDRAWS=1` (set by the
  test harness) each output has a second damage tracker that follows what's drawn
  (`check_redraws`): on each render pass and every 50 ms, an output no redraw was asked for
  that has damage anyway is suspect, and Pila panics if no request explains it within 20 ms (a
  request may come later in the same iteration). Not while animating or a lock is pending, while
  an animation whose time is up is still held (`Pila::animations_held`: `step_animations` hasn't
  asked for its last frame yet), nor while an idle callback of `defer_redraws`
  hasn't run (`Pila::deferred_redraw_idles`): those redraws are asked for,
  not yet queued. The state counts both too: such an animation as `running`, such a redraw as
  `redraw_pending`, so a test's `settle` doesn't stop before them. With
  `PILA_HEADLESS_CHECK_INVARIANTS=1` (also set by the harness), every 50 ms
  `broken_invariant` checks that input can't get stuck: no popup grab (pointer or keyboard; an
  input method's keyboard grab isn't one) outlives every menu (its candidate window isn't
  one), and the keyboard isn't on a layer surface that takes none; one broken for 500 ms (a
  client may be slow to destroy a dismissed menu) is a panic. A new kind of grab or focus
  Pila takes gets its invariant there.
- **Nested-only (winit):** named cursor shapes are drawn by the host (`apply_cursor`),
  but while the cursor is magnified (`shake_cursor.rs`) Pila draws them from its own theme
  and hides the host's; client cursor surfaces are drawn by Pila (`cursor.rs`). Winit doesn't report the pointer
  leaving the window, so a drawn cursor lingers at the edge. Keys held when the host takes
  focus away (e.g. GNOME's Alt+Tab) never get a release (Smithay's winit backend drops
  winit's synthetic ones), so `WinitEvent::Focus(false)` calls `Pila::release_all_keys`.
  The dmabuf global is created in `winit::init_dmabuf`. Input is absolute motion only,
  mapped onto the single output; relative motion is derived from it. When the host's
  connection fails (the host session ended or crashed), winit ends its loop and Smithay's
  source removes itself without an event; `winit::HostWindow` wraps it and turns that end into
  `Nested::Ended`, which stops the loop, and `main` then fails ("lost the host compositor",
  `winit::host_lost`). Without it Pila ran on with no window while winit's thread watching the
  host's socket spun on the dead connection, printing `Io error: Broken pipe` nonstop.
- **Cursor** (`cursor.rs`): a client's cursor surface (`wl_pointer.set_cursor`) is drawn at
  `surface_position` (pointer minus hotspot, on top as `Kind::Cursor`) and gets frame callbacks
  like any surface; `handle_commit` moves the hotspot with the buffer's offset on every commit.
  Named shapes (cursor-shape, Pila's own grab cursors) come from `CursorTheme` (one,
  `Pila::cursor_theme`, for every backend and capture) where Pila draws them: bare metal always,
  nested and headless only magnified; `Pila::frame_input` leaves `FrameInput::named_cursor` empty
  where the host draws it (`Backend::draws_cursor`). The theme and size are `[cursor]`'s, else
  `XCURSOR_THEME`'s and `XCURSOR_SIZE`'s (`CursorTheme::load`); set in the config, `session_env`
  hands them to what Pila starts and `export_environment` to the session bus, and a reload that
  changes them loads a new `CursorTheme` and exports them again (`apply_config`). A thread reads the xcursor theme at
  startup, every shape at every nominal size up to `LARGEST_IMAGE` times the size, and
  until then the built-in arrow stands in (one buffer, kept: a new one each frame would be
  damage), so no frame reads a file. `get` picks the image for a buffer scale, `get_magnified`
  the smallest at least as large as a magnified cursor shows.
- **Shake to find** (`shake_cursor.rs`, `[cursor]`): every pointer motion that moves Pila's
  pointer (`Pila::pointer_motion`, not a locked pointer nor a warp) goes to `ShakeDetector`,
  which keeps the last second of motion as the points where it turned (a move less than a
  right angle off the last stretch moves its end instead) and calls it a shake when the way
  travelled is over 4 times the diagonal of its box and the box is at least 60 px across. A
  shake starts or continues `Phase::Growing` (1 to `shake-magnification` over 500 ms of
  shaking, frozen at the last shake: frames come from the motion, which redraws the cursor);
  a calloop timer `HOLD_MS` after the last shake starts `Phase::Shrinking`, an `Animation`
  that `step_animations` steps and `animating` counts, on the pointer's output, locked or not.
  `[animations]` `speed` scales all three, and with animations off growing and shrinking are
  instant. `Pila::cursor_magnification` is the factor (`None` at rest), taken into
  `FrameInput::magnification`: `render::pointer_elements` draws a named cursor from
  `get_magnified` at the theme's size times it and a client's surface through
  `RescaleRenderElement`, both around the hotspot, which stays at the pointer (hit testing
  doesn't change). Starting and ending the magnification calls `Backend::cursor_changed`
  (nested, who draws a named shape changes). A bigger cursor than the cursor plane is
  composed (Smithay falls back by itself). Turning it off in a reload puts the cursor back at
  once (`reset_shake_cursor`).
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
- **Bare metal (udev):** `udev/mod.rs` has the session, the GPU (its DRM device and connector
  scanner a `GpuDevice` in `UdevData::devices`, outputs keyed by `OutputId`: its node and the
  CRTC) and `UdevData`; `outputs.rs`
  connectors and outputs, `draw.rs` frames and vblanks, `recovery.rs` DRM failures,
  `input.rs` and `gamma.rs` the rest. It renders with one GPU, the seat's
  primary, whose renderer a `GpuManager` owns (`UdevData::gpus`, every GPU's renderer) and lends
  for each use (`primary_renderer`; outside the backend, `Backend::renderer`'s `RendererRef`,
  `None` if it can't). The seat's other GPUs (`secondary.rs`, `docs/MULTI_GPU.md`) only scan out,
  and only with `other-gpus = true` (off by default: untried on real hardware; `apply_other_gpus`
  opens or drops them when the setting changes):
  opened at startup, on `UdevEvent::Added` and after a VT switch back (`add_gpu`; dropped on
  `Removed`, their outputs as unplugged), each a `GpuDevice` with its own DRM events; their
  outputs are set up with their GPU's own renderer and drawn with a `MultiRenderer` from the
  primary (`setup_renderer`, `draw_renderer`: `OutputRenderer`), Pila's elements wrapped in
  `PrimaryGpuElement` (drawn on its `GlesFrame`, never scanned out directly); their captures are
  drawn from the scene, not blitted from the frame. The frame is copied through a dmabuf the
  other GPU imports or, failing that, through memory; always through memory with a software
  renderer, which would crash reading an imported buffer, and which `gpus.rs` (`GpuApi`, around
  Smithay's `GbmGlesBackend`) knows by its `GL_RENDERER` name too: llvmpipe on a KMS device
  isn't a software EGL device, which is all Smithay asks. The kernel numbers connectors of a
  kind across GPUs, so names don't clash; should one, it gets a number (`unique_name`). Outputs are placed by their `[[output]]` config (mode, scale, position, transform) or else left to right, each where the one before ends in the `Space` (its logical width rounded up, as `Space::output_geometry` does). Pila draws named cursors from the xcursor
  theme (`cursor::CursorTheme`; read by a thread at startup, the built-in arrow until
  then, so no frame reads a file). Redraws are per output (see "Redraws are on demand"):
  a request marks its outputs (`OutputSurface::ask_redraw`) and schedules one idle
  `render_all`, which renders those; an output with a frame in flight renders at its vblank
  (`frame_finish`), which also sends presentation feedback with the kernel's timestamp. A
  redraw that finds nothing new queues no frame, so nothing would pace the next: a client
  committing nothing in a loop of frame callbacks (no new picture: no buffer or one without
  damage, `redraw::brings_content`) or the pointer moving with the cursor hidden would make Pila
  redraw as fast as it loops. After any such redraw, whatever was asked, the output waits until half a period before its next
  vblank (`draw::idle_until`, `OutputSurface::idle_until`; a one-shot timer,
  `schedule_idle_redraw`, draws what was asked meanwhile) before the next that may bring
  nothing (`draw::idle_wait`): at most one per refresh. What's asked says what it brings
  (`redraw::Content`, the most of the requests since the last redraw: `redraw_content`): a
  change of Pila's own (`Sure`: a window moved, an animation, a drawn cursor) never waits; a
  client's new picture (`Client`) doesn't, once per wait (and `Sure` not after
  `SURE_STREAK` redraws in a row found nothing, a bound on a loop of Pila's own requests): if that redraw finds nothing new
  either (`idle_streak`: two in a row, a covered surface's commits paced by its fifo barriers,
  say), it waits too, until the wait is over. An output with a frame in flight (its vblank draws it)
  or a lock waiting for its frame never waits. Past those waits a frame is drawn late, its
  margin before the next vblank (`render-late`, on by default; `udev/render_late.rs`,
  `draw::hold_late`), but only in a steady scene: `OutputSurface::render_at` holds the output
  and a timer (`schedule_late_render`) draws it then, so what clients and input bring meanwhile
  makes that vblank. The margin is the worst of the output's last frames' readiness
  (`RenderTiming`: from `prepare` to the later of the commit queued and the GPU's fence, which
  `Pila::watch_gpu` reports through `Backend::gpu_done`; frames that were a client's buffer as
  it is kept apart, the worst of either counting, so the first frame drawn after direct scanout
  isn't given a handed-over buffer's margin), plus the other outputs' worst (drawn first, they
  hold it up), plus `render_late::SLACK`, never under half a period (`MIN_MARGIN_DIVISOR`).
  Nothing is held before a frame was measured, after a composited frame with no fence, with a
  vblank older than a second (the rounded refresh drifts), after a torn frame
  (`OutputSurface::torn`), with adaptive sync, for the first frame after a modeset, while a lock
  waits, while Pila animates, or for 300 ms after a window or layer surface appears or a window
  draws itself at a new size (`Pila::start_settling`, from `animate_open`, `layer_committed` and
  `geometry_committed`'s `DrawnSize`, animated or not: `render_late::settling`): apps are busy
  on the GPU then as recent frames don't foretell, and an animation of Pila's shows at the same
  vblank either way. A held frame is let go at once if the scene stops being steady while it
  waits (`render_late::still_held`), and one that still comes late or misses a refresh
  (`queued_held`) turns it off for that output for two seconds. Leaner margins, and one output's
  alone, left apps busy on the same GPU making held frames late on a TTY: the module docs say
  how. A held frame counts as on its way for missed refreshes (`back_to_back`). A frame that
  fails at the DRM level (render or queue: no DRM master yet after a VT switch, a monitor beyond
  the link's bandwidth) blocks only its own output until a per-output timer retries it
  (`recovery.rs`: 100 ms doubling up to 10 s, logged at the 1st, 2nd, 4th… failure, reset by
  a queued frame, `ActivateSession` or the output coming on; the retry takes DRM master
  first), so one broken output never stops the others. Setting a CRTC up (`enable_output`,
  `set_output_power`) goes through `initialize_with_fallback`: if the mode asked for fails
  (Smithay's `initialize_output` test-commits it, so a mode beyond the link's bandwidth fails
  there), the connector's `fallback_order` follows (same size at lower refresh, then smaller
  sizes, nothing heavier, at most 8 tries), and the one that works becomes the output's mode;
  after a power-on, `output_power.rs` then calls `Pila::fit_to_outputs`. A mode change asked
  for explicitly (`apply_output_config`) doesn't fall back: it fails and says so. After every
  connector scan (a udev change event comes with it) `retrain_bad_links` looks for enabled
  outputs whose `link-status` the kernel set to bad (DisplayPort link training failed, e.g.
  after a resume or through a dock), which show nothing until the compositor acts: on an
  atomic device a commit sets it back to good with `ALLOW_MODESET`, so the kernel sets the
  same mode again (Smithay doesn't handle the property); otherwise, or if that fails, the
  output is powered off and on. The session lock is
  confirmed once every output has *shown* a locked frame. `PauseSession` (VT switch away)
  releases held keys; `ActivateSession` takes DRM master, resets each device (`activate(true)`:
  every connector off, one modeset, since another compositor may have left them on other
  CRTCs), clears each output's recovery and frame in flight (`first_frame` again), re-applies
  gamma (`reapply_gamma`), then scans the connectors, opens GPUs added meanwhile
  (`apply_other_gpus`) and re-applies the outputs, since a monitor plugged in meanwhile can't
  come on without DRM master (`enable_output` waits). Libinput devices get keyboard LEDs and the
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
  per further stretch and ending on lift), unless locked or shortcuts are inhibited (a swipe
  begun before a lock runs nothing once locked). A gesture's begin and end redraw every output;
  its updates, at the touchpad's rate, only a step Pila acts on (`on_gesture_swipe_update`):
  a client's are the client's to show.
  Output transforms (rotation) are the `Output`'s: Smithay rotates the rendered frame and
  the `Space` swaps a turned output's logical size, so Pila only sets it (and sizes output
  captures accordingly); clients are told `Normal` and draw upright. Still missing: touch.
- **Render loop** (`winit::redraw`): collects `Pila::scene()` (windows front to back with
  their title bars) and the cursor *before* borrowing the backend (`Pila::frame_input`; built
  once per pass and shared by the outputs drawn in it, `udev/draw.rs::render_all` and
  `headless.rs`'s, after each one's commit timers applied, its cost counted in the first
  output's frame; and once for the captures and screencopies of an `after_frames`). An
  output's elements leave out the windows and layer surfaces that don't touch it
  (`SceneWindow::bounds`, from `window_visual_rect`; an animating one by where its effect draws
  it, `effect_rect`; a sliding one never), builds elements with
  `render.rs` (backend-neutral, but on `GlesRenderer`: `PilaRenderElement` is concrete,
  since Pila's custom shaders need `GlesFrame`), and renders them: nested and headless
  through the damage tracker's `render_output` directly, bare metal through its `DrmOutput`
  (`render_frame`, `queue_frame`). Smithay's `space::render_output` isn't used because it can't
  interleave title bars with windows. Afterwards `frame.rs` (backend-neutral) records each
  drawn surface's primary output from the render states, sends presentation-time feedback
  once the frame is submitted, and sends frame callbacks only to surfaces whose primary
  output it is (surfaces shown nowhere: once a second, `HIDDEN_FRAME_THROTTLE`, so a
  covered animating client doesn't spin; `schedule_hidden_frames`, from `after_frames`,
  asks for a redraw after that second while such a surface waits, since nothing
  else may redraw; a captured window's hidden surfaces, off-space ones included, get theirs
  from its capture instead: see "Screen capture"); frame callback times and
  feedback both use `Pila::clock` (CLOCK_MONOTONIC). Once per event-loop
  iteration, however many outputs were drawn, `Pila::after_frames` (`state.rs`, from
  `main.rs`'s `event_loop.run` callback, a flush after it) tells the idle notifier of the
  iteration's input (once: each call re-arms every timer), saves sessions and sends keyboard
  layout events, then, when an output was drawn, `Pila::housekeeping` is set or a capture is
  due: applies the commits fifo unblocked, captures and screencopies (only when an output was
  drawn or `capture_due`; see "Redraws are on demand, per output"), `space.refresh()`, taskbar
  updates, IPC events, idle inhibitors, `popups.cleanup()`, animations, hidden surfaces' frames
  and slide textures (`prepare_slide_textures`); else only the IPC events already due
  (`send_ipc_events_due`). Buffer age (for partial repaints) needs the window's EGL
  surface made current first (`winit::make_surface_current`); Smithay's `bind` is lazy,
  and querying without it fails with `EGL_BAD_SURFACE` every animated frame.
- **Redraws are on demand, per output** (`redraw.rs`): nothing redraws an output but a
  request for it. `queue_redraw_output` asks one output for something new to show
  (`request_redraw_as` with another `redraw::Content`, see "Bare metal"), `queue_redraw_rect` the outputs a
  global rectangle overlaps, `queue_redraw_window` those a window's frame, title bar, shadow
  margin and popups touch (`window_visual_rect`; no margin for a maximized or fullscreen window,
  which has no shadow and reaches its output's edges: each of its frames would redraw the
  outputs next to it), `queue_redraw_pointer` those of where the
  pointer was and is. `queue_redraw` asks every output, for what's global: the config,
  outputs, the lock, workspaces, focus (`focus_changed`: title bars and shadows show it), a
  key Pila acts on, a click, a gesture's begin and end, an animation starting (its steps ask
  only for the outputs it shows on, `animation_outputs`). Who asks: a commit asks for its surface's
  outputs before and after it (`redraw_committed`; `surface_outputs` resolves a subsurface,
  popup, window, layer, lock, cursor or drag-icon surface, and an unknown one asks every
  output); every change to the `Space`'s windows goes through `map_window`/`unmap_window`/
  `raise_window`, which redraw where the window was and is (a test reads the sources for a
  direct `space.map_element`, which moves a window and redraws nothing); input asks in
  `process_input_event` (a key or scroll a client gets is the client's to show); a destroyed
  surface or popup, a layer surface removed, a title, a lock, an output powered on ask for
  theirs. `after_frames`' housekeeping (`space.refresh`, taskbar, IPC events, animations) runs
  after any client dispatch or input (`Pila::housekeeping`) even when no output was drawn: the
  change may show nowhere (a minimized window's title). The pointer moving is the exception: it
  comes up to a thousand times a second and changes little of that, so it asks only when the
  keyboard focus or the output under the pointer changed (`Pila::motion_watch`); a window it
  moves draws a frame, which does the rest. Captures run only when an output was
  drawn or `Pila::capture_due` (a commit that shows on no output, a hidden window's that may be
  captured; a capture client asking for a frame), not with every dispatch. State changed outside a client
  dispatch or input (timers, `insert_idle` callbacks) must ask for its redraws. The headless
  check (`PILA_HEADLESS_CHECK_REDRAWS`, every integration test) panics on an output that
  changed with none asked. Clients are flushed after every event-loop iteration (`main.rs`),
  not only on redraw. calloop doesn't wake the loop for pending idle callbacks: they run after
  the next event, however late. So a redraw request pings `Pila::wake` (bare metal, the
  redraw itself is an idle `render_all`), and anything else inserting an idle callback
  outside an event source (e.g. from `after_frames`, the `event_loop.run` callback) must
  ping it too. Redraws requested from `after_frames` are held back into an
  idle callback plus the ping (`defer_redraws`): nested, the winit source has already checked for redraw
  requests by then. Smithay calls `cursor_image` with the pointer locked, so its redraw
  (where the pointer is) goes through an idle callback too; the same named or hidden cursor
  again (some clients set it on every motion) asks for none (a surface may bring a new hotspot).
- **Direct scanout** (`udev/draw.rs::frame_flags`; bare metal, `direct-scanout`, on by
  default): a frame that would be one client's buffer and nothing else (a fullscreen video or
  game, a maximized window with no title bar on an output with no bar, a wallpaper alone; the
  cursor on its own plane) goes to the screen as that buffer, on the primary plane, and nothing
  is drawn. Smithay's `DrmCompositor` decides, frame by frame: the last element left once those
  hidden behind opaque ones are dropped, opaque and covering the output (or anything, on a black
  clear color: the lock screen's surface), with nothing drawn above it; it tries the buffer on
  the kernel (an atomic test) and draws the frame if it's refused. It only takes a client's
  dmabuf that names the output's GPU, which `dmabuf_imported` sets to the primary's
  (`Backend::dmabuf_node`) unless the client named one; outputs on another GPU never do (their
  elements have no `underlying_storage` either). The frame flags allow the primary plane,
  whatever the buffer's format, and the cursor plane; not overlay planes (what they take
  differs by driver, and they'd be for a window among others, which saves little). A capture
  of such a frame draws the buffer (Smithay's `blit_frame_result`). So that the client's buffer
  is one the plane takes, a surface an output showed as it is, or tried to and was refused, is
  told that output's formats first (per-surface dmabuf feedback: `OutputSurface::scanout_feedback`,
  the primary plane's formats among those the renderer reads, as a scanout tranche;
  `frame::send_dmabuf_feedback` after each frame, sent when it changes), and the usual ones
  again once it's drawn among other things. Such frames are counted
  apart in the frame statistics (`direct`) and an output starting or stopping them is logged
  (`direct scanout`, `pila::frames`, with the time that frame's render and commit took and
  `others_pending`, the device's other outputs with a commit on its way, and the format and modifier
  of the client's buffer and of the swapchain (`OutputSurface::client_format`, `swapchain_format`)).
  Why one isn't is logged too, each when it changes: an output enabled with or without scanout
  formats for clients (`direct scanout formats for clients`), a client's buffer Smithay tried and
  couldn't show (`direct scanout refused`, `draw::refused_scanout`: its format and modifier, and
  `ScanoutFailed` when GBM or the kernel refused it, as amdgpu does a buffer whose DCC isn't
  displayable), and a surface told the plane's formats or back to the renderer's (`dmabuf
  feedback`, `scanout`). With Mesa 26.3 (radeonsi) EGL clients (kitty, weston-simple-egl) keep
  their render-only DCC modifier after a scanout tranche, so they're drawn, not shown as they
  are.
  The swapchain is opaque (`COLOR_FORMATS`: XRGB8888 first) because Smithay adds a client's buffer
  shown on the primary plane as the opaque variant of its format (ARGB8888 as XRGB8888): with an
  ARGB8888 swapchain the plane's framebuffer changed format at every switch between the two, and
  amdgpu (`should_reset_plane`) resets a plane whose framebuffer changes format, and a reset waits
  for every CRTC's flip (`amdgpu_dm_atomic_check`, `do_aquire_global_lock`): 11 to 14 ms with the
  event loop held on Renoir, leaving direct scanout with another monitor's flip pending. The
  kernel's atomic debugging showed the reset ("Disabling/Enabling DRM plane") at both switches and
  at Smithay's test of the client's buffer; the buffers themselves had the same format, modifier,
  tiling metadata, size and alpha (the log's `client` and `swapchain` are the buffers' formats, not
  the framebuffers'). With the opaque swapchain the same switch takes 0.06 ms with
  `others_pending=1` (two monitors, Renoir). Bare metal only: `scripts/check`'s fullscreen scenario
  reports the count on a TTY.
- **Tearing** (`tearing.rs`, wp-tearing-control-v1; bare metal, `tearing`, on by default): a
  surface's presentation hint is double-buffered surface state (`TearingHint`, applied on
  commit; destroying the object goes back to vsync on the next commit; a second object for a
  surface is `tearing_control_exists`). `Scene::tears` decides per output: not locked, no
  workspace slide or window menu, and the window in front there is fullscreen over all of it and
  `SceneWindow::tearing` (its main surface asks, and the config allows it). `render_output` passes
  `PresentationMode::Async` to `render_frame` then, and Smithay's `DrmCompositor` (Pila's fork:
  Smithay's pull request #1948) flips asynchronously only when nothing but the primary plane
  changed in a way the last flip shows the plane took (the pointer moving over the game is a
  normal flip), and goes back to a normal flip, until the plane changes, when the driver refuses
  one; a composited frame tears too, not only direct scanout. In `frame_finish` a torn frame
  (`with_pending_frame`'s mode) misses no refresh and isn't late, its presentation feedback has
  no `Vsync`, and it leaves `last_vblank_time`, the grid the next vblank is reckoned from, as it
  was (so a frame asked for while a torn one was in flight is due as if none had been, not a
  period after that vblank: `draw::in_flight_shown`); the frame statistics count it apart (`torn`). The headless backend can't tear: the state
  tells what the DRM backend would ask (`tears` per output).
- **Frame statistics** (`frame_stats.rs`, `RUST_LOG=pila::frames=debug`): every backend calls
  `Pila::record_frame` with a frame's CPU time (logged per output every 5 s: frames, average
  and worst, missed refreshes; each frame over half the period on its own); winit and udev
  also call `record_missed` with the refreshes missed. Bare metal counts them from the DRM sequence of
  the vblanks that showed frames while an animation runs and the output has a frame on its
  way (`missed_refreshes`, `back_to_back`: an output resting while another animates misses
  nothing); nested
  estimates them from gaps over 1.5 periods (`NestedPacing`: a frame within half a period of the
  last takes the next refresh, and the gap after it counts from the frame before, less that one;
  a host that renders late draws the first frame after a pause at once and the next late). Screen captures are recorded apart
  (`record_capture`, a `captures` line), the blit into them subtracted from the output's
  frame. Clients' shm buffers copied at their commits ("Uploads") are recorded
  apart too (`record_upload`, an `uploads` line: how many, average and worst CPU). The first frame after an output is set up (`OutputSurface::first_frame`: swapchain
  and modeset) is logged on its own and not counted. A redraw that found nothing new on an output
  (no frame queued: a commit with no new picture, the pointer moving with no cursor drawn) is counted
  apart, `idle` and its summed CPU `idle_cpu_ms` (`record_idle_frame`, udev and headless), and a
  frame that was a client's buffer as it is in `frames` and apart, `direct`
  (`record_direct_frame`, udev); an
  output with only those still reports. On bare metal a frame with something new asked for in
  time for a vblank (`frame_stats::IN_TIME`, 4 ms or half a period, before it; with a frame in
  flight, the vblank after the one that really showed it: one shown late doesn't make the next
  late) but shown at a later one counts its refreshes as missed,
  animation or not, and is counted `late` (`late_refreshes`, from `OutputSurface::request`, the
  first such request since the last redraw (one that finds nothing drops it); not with adaptive sync, nor for the first frame
  after a modeset). Each late frame is logged with what made it late: held back by the wait
  (`held`), drawn late (`drawn_after_ms`) or shown late after being drawn in time
  (`shown_after_ms`: the GPU, the commit). The event loop's own work after each iteration
  (`Pila::after_frames`, captures left out), which runs as often as input and clients bring
  events, not once per frame, is a `loop` line (`record_loop`): iterations, those that did all
  of the housekeeping, and its CPU time, summed and worst; `state_json` has the totals
  (`event_loop`). The GPU's part of a frame is timed apart, every backend alike: the frame's
  sync point, if the renderer gives it a native fence (`GpuFrame::new`, the swapchain's on bare
  metal; none for direct scanout, nor on software rendering, logged once), is watched by an
  event source (`Pila::watch_gpu`), and once ready its signal time, the kernel's stamp read with
  `SYNC_IOC_FILE_INFO` (`fence_signaled_at`), less the time the frame was submitted is its GPU
  time (`gpu_frames`, `gpu_avg_ms`, `gpu_max_ms`; one over half the period is logged on its own,
  and every frame's at trace level, `GPU done`):
  its own GPU work and whatever the GPU had queued before it. A fence older than the frame, or
  the one measured last (Smithay hands back the last frame's when the primary plane had nothing
  to draw), is an earlier frame's and measures nothing (`gpu_time`). Headless waits for the GPU
  anyway (its CPU time includes it), nested and bare metal never do. On bare metal the report
  also has the frames held to render late (`held`), how far before their vblank (`margin_*`,
  `render_late::Hold`) and how long before it they were ready (`spare_*`: from `RenderTiming`'s
  `queued` for a client's buffer, else its `gpu_done` through `Backend::gpu_done`; zero for one
  ready after it), and the frames' latency, from the first
  redraw asked for with something new (`OutputSurface::request`) to the vblank that showed it
  (`latency_frames`, `latency_avg_ms`, `latency_max_ms`). `scripts/check` judges these
  lines and reports the idle redraws per output, the GPU time and latency per output and the
  loop's work.
- **Pointer grabs** (`grabs/move_grab.rs`, `grabs/resize_grab.rs`): Mod+drag installs the
  move/resize grab *before* the press is
  forwarded, so the client never sees that click. Move/resize grabs show their own cursor
  via `Pila::set_cursor`, which must be called *after* `set_grab` (`Focus::Clear` resets the
  cursor), and reset it in `PointerGrab::unset`. A resize keeps one configure in flight for an
  xdg window: a motion while the app hasn't acked the last one only sets the pending size, and
  its ack sends it (`XdgShellHandler::ack_configure` → `Pila::resize_acked`); the configure that
  ends the resize goes regardless. Client-requested move/resize goes through
  `check_grab` (`handlers/xdg_shell.rs`), which requires a live implicit grab on that
  client's surface. Popup grabs (menus, `XdgShellHandler::grab`) use Smithay's
  `PopupKeyboardGrab` and, for the pointer, `MenuPointerGrab` (`grabs/menu_grab.rs`): Smithay's
  `PopupPointerGrab` but over an input method's candidate window, which gets the pointer and
  its clicks without dismissing the menu (a text field in a popover types through it); they are the only keyboard grabs Pila takes, but
  not the only ones there are: an input method's holds the keyboard while it's active (see
  "Input methods"), so a menu's is told apart by its type (`Pila::keyboard_popup_grab`,
  `Pila::pointer_popup_grab`), and Pila only ends a menu's. A menu opened while an input
  method holds the keyboard takes the pointer only (a device has one grab, and Smithay can't
  give the input method's back): the keyboard stays on the window, and the input method
  passes on the keys it doesn't use; such a menu's grab is kept
  (`InputState::pointer_only_menu`), as it may be another surface's than the keyboard's (a
  panel's). A popup grab ignores focus changes, so `focus_window` dismisses it first
  (`dismiss_popup_grab`, which closes a pointer-only menu through its grab and leaves the
  focused surface's popups alone, and never closes the input method's candidate window), and
  `focused_window` resolves a focused popup to its window.
  Smithay ends a popup grab only at the next input event and spends it (a motion that ends
  the pointer grab moves nothing), so after a popup is destroyed `end_finished_popup_grab`
  (idle, after the request) ends a finished grab at once: the manager's cleanup, a motion
  where the pointer is (passed on if a menu is still open), then a fresh pointer focus. Once
  the pointer's grab is over, there or by a click outside the menu (`pointer_button` calls it
  then, when the button ended a menu's pointer grab), it ends the keyboard's grab too, and
  gives the keyboard to the most recent window if it's on nothing, on a layer surface that
  takes none, or a menu of one (a dock's), or on a menu that's gone; still on a menu of a
  window (its keyboard grab was gone first: an input method took the keyboard and let it go),
  to that window.
- **Window lifecycle:** `new_toplevel` pushes the window onto `Pila::windows.unmapped` (not the
  space). A client unmapping it (a null-buffer commit, `unmap_if_buffer_removed`) takes it
  out of the space, taskbars (`remove_foreign_toplevel`, which also stops its capture
  sessions), Mod+Tab and focus, and puts a new `Window` for the toplevel in `unmapped`:
  it maps again like a new window. Smithay resets the toplevel's state in that very commit
  (xdg-shell discards title, app id and decoration mode on unmap), so the unmapping commit
  never answers as an initial commit: the client's next one does, and the decoration mode
  it had (`ChosenDecoration`) goes out again with that configure, since a decoration object
  outlives the unmap and its client doesn't ask again. A destroyed decoration object
  (`XdgDecorationHandler::decoration_destroyed`, called by Smithay on the commit that applies
  it, the mode already dropped) forgets it: the client draws its own from then on.
  `handle_xdg_commit` sends the initial configure, then on the first commit with a
  buffer moves it into `space` via `place_new_window` (centered on the pointer's output, or
  where its session restores it, see "Session management"; cascaded by `CASCADE_STEP` if
  the spot is taken). Lookups that must find unmapped windows
  use `window_for_surface`, which searches both.
- **Client rendering** (`handlers/compositor.rs`, `wait_for_client_rendering`): a
  pre-commit hook on every surface blocks a commit whose new dmabuf the client's GPU is
  still writing (`Dmabuf::generate_blocker`, polling its implicit fence). The commit applies
  when the fence signals (`blocker_cleared`, whose commits ask for their redraws), so a slow client delays
  its own frame and never Pila's. Commits of one surface still apply in order. With
  linux-drm-syncobj (explicit sync; offered where the backend has a DRM device whose kernel
  has `syncobj_eventfd`: bare metal and headless with a GPU, not nested) a commit's acquire
  point takes the place of the fence, and Smithay signals the release point when the buffer
  is dropped. Pila keeps no buffer of its own: animation snapshots are textures.
- **Uploads** (`upload.rs`, `Pila::upload_committed`, from the commit handler before
  `redraw_committed`): a client's shm buffer reaches the GPU as a copy the CPU makes
  (`glTexImage2D`, `glTexSubImage2D` of its damage), which Smithay makes when a frame builds its
  elements (`import_surface`). Made in the frame, a whole-screen picture put milliseconds on it:
  swaybg's first buffer cost 11 to 19 ms of a frame's CPU on Renoir's bare metal (a missed
  refresh at every start), a fullscreen foot scrolling on 2560x1440 about 3 ms of every frame
  against kitty's (dmabuf) 1 ms; and a frame held to render late (`udev/render_late.rs`) left
  that copy its margin to fit in. The commit makes it instead (`import_surface_tree`, with the
  renderer `Backend::renderer` lends: on bare metal the primary's `GlesRenderer`, the one
  frames build their elements with, so Smithay's per-renderer texture is found there and not
  copied again). Only for what a frame would copy anyway: a surface with an shm buffer that has
  no texture yet (`needs_upload`), shown on an output (`surface_outputs`: not minimized, not on
  another workspace; while locked, only a lock surface; a sync subsurface waits for its
  parent's commit), and once between frames (`UploadedAfter`, by `Pila::frames_drawn`, which
  `post_render` counts): a client committing faster than the outputs refresh leaves the rest to
  the frame, which gets the damage since the last commit copied (`damage_since`). Smithay
  (Pila's fork) copies in strips of at most 128 KiB, a new texture allocated empty first: one
  call for a whole 1920x1080 buffer took radeonsi's slow path (a staging buffer of its size,
  allocated anew while the last is busy), 10 to 22 ms about one time in three; `scripts/check`
  fails a copy over 8 ms. Dmabufs are imported as before (no copy). The copy's time is counted apart, an `uploads` line on
  `pila::frames` (`record_upload`), and `early_uploads` in `state_json` counts those commits.
  On headless, whose frames wait for the GPU, the GPU's part of the copy still lands in the
  frame that first draws the textures.
- **Surface limits** (`surface_limits.rs`): no size or place a client gives a surface goes past
  `MAX_EXTENT` (32768 logical px; buffers in pixels), so no sum of a position and a size
  overflows, in Pila's layout and drawing or in Smithay's (bounding boxes, popup placement).
  The first pre-commit hook on every surface (`check_commit`) walks its whole tree from the
  root, since a parent's commit applies its synchronized subsurfaces' positions: each
  subsurface's place relative to the root, buffer sizes and offsets, viewport destinations,
  xdg window geometry, layer-shell sizes, margins and exclusive zones. What's past the limit
  is a `wl_surface` protocol error (`invalid_size`, logged), and is cleared or reset in the
  pending state, so the commit that still applies before the client is gone is within it;
  xdg min/max sizes are clamped silently (a large maximum can mean "none"). Popup
  positioners apply at `new_popup`/`reposition_request`, not at a commit, and are checked
  there (`check_positioner`). Input and opaque regions aren't errors (`INT32_MAX` is a common
  "everything"). Smithay builds a layer surface's `Size` from the u32 it gets, and past
  `i32::MAX` its debug assertion panics before Pila sees it (debug builds only; release
  builds reach the check). A new request carrying a client's size or position gets a check
  here. Outside surfaces: a screencopy region past the limit fails its frame (`position_ok`,
  `size_ok`); points that only get added to others (a cursor's hotspot, where an app asks for
  its window menu, a drag icon's accumulated offset) go through Smithay's `Point` arithmetic,
  which saturates, and are tested far off (`tests/surface_limits.rs`).
- **Content update pacing** (`commit_pacing.rs`, wp-fifo): Smithay blocks a commit waiting
  on a barrier; Pila signals barriers. A commit that sets one registers its surface
  (`commit_paced`), `post_render` signals those shown on the output just drawn, and
  `after_frames` applies what that unblocked (its commits' redraws deferred, `defer_redraws`). So a client gets at most
  one content update per refresh. A barrier a frame didn't signal in a second is signaled by a
  timer (`signal_old_barriers`) wherever the surface is, so hidden clients and clients on an
  output turned off carry on, once a second. No damage means no flip: a commit that changes
  nothing frees its barrier at the next redraw, which finds nothing to show; after one such
  redraw the next waits (bare metal: until half a period before the vblank, `draw::idle_wait`),
  so a looping client still gets about one update per refresh.
  wp-commit-timing: `hold_commit_until_timestamp` (pre-commit hook; Smithay's unmanaged mode)
  blocks a commit with a timestamp and arms one timer for the earliest deadline, a period
  before it. The redraw that wakes builds a frame, and before building it
  `signal_commit_timers` applies what will be due when it shows (`Backend::next_frame_time`:
  the next vblank from the kernel's timestamps, nested and headless a period from now).
  At the deadline itself anything left applies whether a frame came or not.
- **Stacking order** is the `Space` element order (back to front); raising =
  `Pila::raise_window` (`redraw.rs`: `raise_element` plus its redraw), ending in `keep_on_top`.
  `focus_window` both raises (with its family, see "Transient windows") and sets keyboard
  focus, then flushes pending configures.
- **Transient windows** (`transients.rs`): a window set on another (`xdg_toplevel.set_parent`,
  xdg-foreign, X11's `WM_TRANSIENT_FOR`) forms a family with it. `focus_window` raises the
  whole family (`raise_family`), each window before its children, so a child is always above
  its parent, even with the parent focused. `map_window` raises the window it moves
  (Smithay's `map_element` does, whatever its `activate`), so whatever moves a mapped window
  calls `keep_transients_above` after it. Minimizing hides
  the descendants too, marked to come back with the parent (`show_window`); one minimized
  alone stays minimized. `move_focused_to_workspace` parks the family, a child opens on its
  parent's workspace unless a rule says otherwise. A drag carrying a window to another
  workspace carries only that window. X11 can make cycles, so walks don't assume a tree.
  A modal child on screen (xdg-dialog, or X11's `_NET_WM_STATE_MODAL`: read from X by
  `X11Surface::is_modal`, set before mapping or through `x11_modal_request`, which tells X)
  takes the focus whenever its parent would get it: `focus_window` starts with `focus_redirect`,
  and a child turning modal while its parent has the focus takes it (`modal_changed`), so clicks, Mod+Tab,
  activation and taskbars all agree. The pointer still reaches the parent: the client blocks
  its own widgets.
- **Window switching** (`switcher.rs`): `focus_window` moves the window to the front of
  `Pila::windows.focus_history` (most recent first) unless a Mod+Tab is in progress (`Pila::windows.switch`,
  a snapshot of that order). `cycle-focus`/`cycle-focus-back` step through the snapshot;
  the switch ends (`end_switch`, which records the choice) when Ctrl/Alt/Super are all
  released (checked after every key in `on_keyboard`), on a click, when a switching swipe
  lifts, on a workspace switch or move, on an activation from IPC or a taskbar, and in
  `release_all_keys`. Unmapped windows are dropped with `forget_window`. Whenever the focus has
  to go somewhere else (the focused window closed, unmapped, minimized or left the workspace, a
  launcher that held the keyboard went away, the session unlocked) it goes through
  `focus_recent_window`: the most recent window on screen in that history, the top one only if
  none is. Never the top one first: an always-on-top window always is. Every change to a switch
  (`cycle_focus`, `end_switch`, `forget_in_switch`) calls `Pila::switch_changed`, and the IPC's
  `switch` subscribers get it at the end of that loop iteration (`send_switch_events`, called
  every iteration: a switch can end where it started, with nothing to draw): its windows' ids,
  the pick's and the output the pointer was on when it started (`Switch::output`), for a
  program that shows it; Pila shows it itself too ("Switcher").
- **Minimize** (`minimize.rs`): a minimized window is unmapped from the `Space` and kept
  in `Pila::windows.minimized` with its location (X11 ones get `_NET_WM_STATE_HIDDEN`).
  `window_for_surface`/`window_for_x11` search it too, and destroyed windows are removed
  (`forget_off_space`). The switch order includes minimized windows: a minimized pick is shown
  (`show_window`) while selected, hidden again (`hide_window`) when the switch moves on, and stays
  when the switch ends. Taskbars, xdg-activation, `pila msg focus` and show desktop bring them
  back too.
- **Workspaces** (`workspaces.rs`, `config.workspaces` of them, spanning all outputs): the
  `Space` holds only the active workspace's windows, those on all workspaces (`stacking.rs`) and
  X11 override-redirect ones;
  the others' are parked in `Workspaces::parked` (back to front, workspace and location in
  user data, X11 ones marked hidden: `_NET_WM_STATE_HIDDEN`). Minimized windows remember their workspace too, so
  "off the `Space`" means `off_space_windows()` (parked + minimized): lookups by surface,
  X11 surface or identifier and taskbar listeners chain it, and destroyed windows are
  dropped from both (`forget_off_space`); both keep location and workspace in the same
  user-data record (`workspaces::leave_space`/`space_location`). Invariant: a window in the `Space` is on the
  active workspace. `switch_workspace` (refused while locked) ends Mod+Tab and popup grabs,
  swaps the windows (a window in Pila's move/resize grab comes along; other pointer
  grabs, e.g. a client's DnD, are left alone), focuses the most recent one of its own there
  (`focus_arriving_window`) and refreshes pointer focus. Mod+Tab only lists the active workspace; activating a window elsewhere
  (taskbar, xdg-activation) switches first (`reveal_workspace_of`). ext-workspace
  (`ext_workspace.rs`, on the `wayland-protocols` bindings): one group with every
  output, workspaces "1".."N", only `activate`, applied on `commit`; diffed by
  `refresh_workspaces` (after a switch, a count change, a window's urgency (`set_urgent`;
  `Urgent` is a workspace state), from `refresh_output_heads` and
  on a client's `wl_output` bind via `OutputHandler::output_bound`).
- **Layer shell** (`layers.rs`, `handlers/layer_shell.rs`): layer surfaces live in each
  output's `LayerMap`, not the `Space`. Front to back: Overlay, Top, windows, Bottom,
  Background; a fullscreen window on top of an output's windows also covers that output's Top
  (`top_layer_covered`, `upper_layers`, `lower_layers`), unless a
  mapped Top surface takes the keyboard exclusively (`takes_keyboard`). All pointer
  hit testing goes through `Pila::under` (layers and windows in that order; `window_under`
  is windows only), and `Pila::scene` renders the same order. Placement, maximize and
  toplevel bounds use `usable_geometry_at` (the output minus exclusive zones); fullscreen
  uses the whole output. When the zone or output size changes, `refit_to_usable_area` re-applies
  fill states and snapped windows' halves and quarters (`refit_snapped`), without raising them
  (`Pila::move_window`: Smithay's `map_element` always raises, `relocate_element` doesn't);
  hidden ones (minimized, another workspace) are fit as they come back. A mapped Top/Overlay surface with exclusive keyboard interactivity
  (`exclusive_layer`) keeps keyboard focus: `focus_window` then only raises. A Top/Overlay
  surface with on-demand interactivity gets the keyboard when it maps
  (`takes_keyboard_on_map`, `MapChange::Mapped`), as on COSMIC; else on a click. Popups may
  have a layer-surface root (`unconstrain_popup`, popup `grab`). Smithay resets a layer
  surface's or popup's role in the commit that unmaps it (null buffer); that commit is never
  answered as an initial commit (`state::commit_map_change` says `MapChange::Unmapped`): a
  layer surface's next commit
  is its new initial one, and a popup can't map again (its parent is gone). A destroyed
  popup's surface starts unmapped (`state::forget_mapped`, in `popup_destroyed`): GTK 4
  opens its next popover on the same surface, and the null commit after the destroy is no
  popup's commit any more, so without it that popover's initial commit read as a hiding one
  and got no configure, while its grab held the app's input.
- **Switcher** (`switcher_view.rs`): Pila's own window switcher, drawn by Pila, that only shows
  the switch `switcher.rs` does (see "Window switching"). `Pila::switch_changed` (and
  `cycle_focus` once the switch is in place) call `switcher_view_changed`: a new switch arms a
  `DELAY` timer (150 ms: a quick tap, there and back, shows nothing), past which it's shown; a
  step or a window gone redraws the panel where it was and where it is (`redraw_switcher_view`),
  and the end hides it and drops its textures. The panel (`switcher_layout`, from the switch as
  it is) is centered on the switch's output: tiles of `COLUMNS` to a row, `ROWS` rows shown
  (`first_shown` keeps the selected one's in view), each a preview over the window's title (or
  its app id), the selected one on the accent color (`[colors] snap-preview`, opaque), on
  `bar-inactive` with a `hover` border, with the windows' corners (`SceneSwitcher::rounding`:
  `RoundedSolid`, the border's a pixel wider; the tiles square). `Scene::switcher`
  (`SceneSwitcher`) is drawn on its output in front of everything but the window menu and the cursor
  (`render::output_elements`), and in captures. A preview is the window's surfaces (no title
  bar, no popups) drawn into an `OffscreenWindow` of its own the size of its place, scaled to
  fit as a whole (`RescaleRenderElement`: a surface element scales only its place) and never
  grown: that texture's own damage tracker redraws only what the window changed, and the output
  sees one texture per tile. Live: while it's shown its windows get frame callbacks from its
  output (`frame::send_frames`), minimized ones and those elsewhere included, and a commit of
  one redraws its tile (`switcher_view_committed`; a window on another output wouldn't redraw
  this one). Titles are rasterized once per text, scale and state (`text::render_title`) and
  kept, with the textures and solid buffers, in `Buffers` while it's shown. `Pila::under` hits
  nothing over the panel, so no client gets the pointer there; a press on a tile
  (`switcher_view_window_at`, in `pointer_button` before a click ends the switch) activates
  that window, which ends the switch on it, and the press and its release go nowhere else. The
  state dump has it (`switcher_view_json`) for the tests; `scripts/apps switcher` drives it with
  real apps on a headless Pila.
- **Overview** (`overview.rs`, the `overview` action, Mod+W): while `Pila::overview` is open the
  active workspace's windows (no override-redirect ones; minimized ones too) are laid out per
  output (`Pila::overview_slots`: each window on `window_output`, a minimized one where it'd come
  back) in the output's usable area less `MARGIN` and the workspace strip (`strip`, a pure
  function: with more than one workspace, a row of thumbnails of the output's shape on top), in rows, one scale for all, never grown
  (`arrange`, a pure function: the row count that shows them largest). The places are computed
  from the windows as they are each time they're asked for, so opening, closing, resizing and
  minimizing meanwhile need nothing of their own; `overview_laid_out` (from `after_frames`)
  redraws the outputs whose places changed (`defer_redraws`) and drops a selection that's gone.
  Nothing is drawn anew: `Pila::scene` gives each shown window a `WindowEffect` whose `FrameMap`
  maps its frame onto its place (`overview_scene_windows`, minimized ones built with
  `scene_window_at` at their remembered place), so it's drawn as an animating window is, with its
  title bar, corners and shadow, and paced by its place's output (`frame::send_frames`); windows
  it doesn't show are left out of the scene. Behind them, above the lower layers,
  `overview_elements` draws the selected window's outline (`snap-preview`) and the dimming; panels
  above the windows stay, and a fullscreen window doesn't cover the Top layer while it's open
  (`top_layer_covered`). Every commit of a shown window redraws its output
  (`overview_committed`: a minimized one has no place of its own on the `Space`). Input:
  `Pila::under` hits only the upper layers while it's open, so no window gets the pointer;
  `pointer_button` gives it the press first (`overview_press`: a thumbnail switches there, a
  window's place is held as a `Press`, a panel keeps its click, anywhere else closes it; the press
  and release reach no app) and the release of a held one (`overview_release`, before the
  suppressed buttons: it picks the window, or, once the pointer went past `DRAG_THRESHOLD`
  (`overview_drag`), drops it: on another workspace's thumbnail `move_window_to_workspace`, the
  path of `move-to-workspace`, takes it there with its transient family, a minimized one staying
  minimized; anywhere else it goes back to its place). A dragged window's `FrameMap` maps it under
  the pointer, in front of the others, shrinking as the drag goes on to fit `DRAGGED_MAX`
  (`dragged_rect`: over `SHRINK_DISTANCE`, by the distance, not by a timer; the point it was taken
  by stays under the pointer); pointer motion
  selects the window under it, and the keyboard filter gives it every key that's no binding
  (`overview_key`: Escape, the arrows through `step`, Enter); `run_action` closes it before any
  action but `Action::keeps_overview`'s (the overview's own, going to a workspace, sending the
  window there, the keyboard layout). Picking is `activate_toplevel` (raised, focused, restored).
  Besides its binding, `hot_corner` (from `pointer_motion`) opens or closes it when the pointer
  reaches an output's top-left pixel that's a corner of the whole layout (no output left of it or
  above it), once until the pointer leaves it (`Overview::in_corner`); not with a button held or a
  grab running (`is_grabbed`), over a fullscreen window, while locked, or with `[overview]
  hot-corner = false`; and `[gestures]` binds `overview open`/`overview close` to 4-finger swipes
  up and down.
  A workspace switch while it's open shows the new windows in their places without a slide
  (`show_workspace`). The strip (`Pila::overview_strip`) shows each workspace's part of the
  output, live: `scene_thumbs` gives each thumbnail its windows as they are (the active
  workspace's from the `Space`, the others' with `scene_window_at` at their parked place; those
  on all workspaces in every one) and the output's Bottom and Background layer surfaces, and
  `overview_elements` draws each into a texture of its own (`render::draw_thumbnail`: at the
  output's scale times the thumbnail's, no blur), kept by output and workspace from one opening to
  the next (`Overview::thumbnails`) with its own damage tracker, so a quiet workspace costs
  nothing; their windows get frame callbacks from the output (`frame::send_frames`, as the
  switcher's), and a commit of one redraws its thumbnail (`overview_committed`). The active
  workspace's is bordered in the accent color, the one under the pointer in `hover`; a press on
  one switches there and it stays open. The strip glides down from above the output and fades. Opening and closing glide (`[animations] overview`, `Overview::glide`, from
  where a glide the other way is): each window's `FrameMap` goes `overview_shown` of the way
  from its frame as it is to its place (minimized ones fading with it), and the dimming and
  outline fade. Closing, it's closed to input at once and the windows glide back from the places
  kept in `Overview::closing`; `end_finished_overview` (from `step_animations`) drops them and
  the windows' textures (`overview_closed`). The glide counts in `animating`/`animations_held`
  and redraws every output (`animation_outputs`); `drop_animations` stops it. The lock closes it, and it doesn't open while locked. The state dump has
  it (`overview_json`).
- **Shortcuts cheatsheet** (`shortcuts_view.rs`): a panel Pila draws with the bindings as
  they're bound: a fixed list of actions looked up in `config.bindings` (`keys_for`: the
  combination to name first, or a run of numbers or arrows as one key, `merged`), an unbound one
  left out; `combo_caps` names the keys with Mod resolved, one keycap each; Mod's drags come
  from `mod_key`. Two layouts (`Heading`): the welcome, the few keys to get going
  (`welcome_rows`), centered under a large title, shown by `main.rs` when
  `config::create_default_file` has just written the config (the first run); and the full
  cheatsheet, `show-shortcuts` (Mod+? by default), in titled sections over two columns
  (`sections`, `columns`), which toggles it. Its output is the one under the pointer when it was
  shown (the first before there's one), and it's centered there (`shortcuts_layout`, computed
  for each scene, so a changed binding shows at once; `panel_size` from the rows alone), shrunk
  as a whole where it's bigger than its monitor with `FIT_MARGIN` around it (`fit`: its content
  is drawn at its layout size, `SceneShortcuts::layout_size`, and shown at the panel's, its
  corners shrunk with it). The
  keyboard filter hides it on any press but a modifier's: a binding still runs (it's hidden
  after the filter, unless the binding is `show-shortcuts`, which hides it itself), any other
  key is suppressed; a Mod tap's action hides it too. A press hides it, and on the panel is
  suppressed (`pointer_button`); `Pila::under` hits nothing over it. The lock hides it, and it's
  never shown while locked. `Scene::shortcuts` is drawn by `render::output_elements` in front of
  the switcher, behind the window menu, and keeps a frame from tearing. Its content (text in
  sizes and weights, `text::render_text`, and keycaps, anti-aliased rounded rects with the text
  blended over them) is one image the size of the panel, drawn on a thread
  (`Pila::draw_shortcuts_text`: ~30 lines shaped and uploaded one by one cost a 7 ms frame; as
  one image drawn off the loop, 2 ms), asked for from `after_frames` only while it has none and
  could (`shortcuts_need_content`: an output or the fonts coming after it's shown; rebuilding
  the key on every turn of the loop cost 18 µs a turn) and by a reload
  (`shortcuts_config_changed`, which makes its background and border anew so that a change of
  corners alone is drawn); one that fails with the fonts loaded isn't asked again until it
  changes. The panel is drawn only once its content is there, never empty. Its background and
  border have the windows' corners (`corner-radius`, `rounded-corners`: `RoundedSolid`, the
  border's a pixel wider). The image and the solid buffers are kept in `Buffers` while it's
  shown. The state dump has it
  (`shortcuts_view_json`: its rows, keys joined by "+", and sections).
- **Simple protocols** (viewporter, fractional-scale, data-control, xdg-foreign, …) live in
  `handlers/protocols.rs`: globals are created in `init_globals`, and only state a handler
  must return is kept (`Pila::globals`). Every Smithay protocol dispatches through its
  user data (`Dispatch2`, one `delegate_dispatch2!(Pila)` in `state.rs`); Pila's own
  protocols implement `Dispatch` on `Pila` with user data of their own. A Smithay global
  that can't take a filter but must be hidden from sandboxed clients is re-created with
  `state::Privileged` as its data (`hide_from_sandboxes`: keyboard-shortcuts-inhibit,
  idle-notify).
- **Clients** (`state::ClientState`): each keeps its security context (sandboxed or not) and its
  process id, read from the socket's peer credentials when it connects (`SO_PEERCRED`, no file
  I/O). A client disconnected for a protocol error is logged as a warning with its pid and the
  error (`ClientData::disconnected`), often the only trace of why a shell went away; a normal
  disconnect is a debug line.
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
  the running slide shows (`snapshot_windows` with the slide). The textures are as large as
  their outputs, and a new one costs the GPU milliseconds, which made a slide's first frame
  miss a refresh: they're kept between slides (`SlidePool`, `Pila::slide_pool`), two made
  ahead per output off any frame (`prepare_slide_textures`, from `after_frames`; none while
  switches aren't animated; those of outputs gone or changed are dropped), taken by a slide
  and given back when it ends (`retire_slide`), a third kept after a switch during a slide. Close and minimize
  ghosts belong to the workspace they left (`Ghost::workspace`): a switch carries them
  off in the leaving picture. A size or place
  change (maximize, fullscreen, snap, and back, a move to another output; not refits nor
  drags) runs `animate_geometry` first (from `set_fill_state_on`, `snap_window`,
  `unsnap_window`, `move_snapped_to`, `move_focused_to_output`;
  not while a window opens, nor inside `without_geometry_animation`, which
  `restore_for_drag` uses): a `Resizing` keeps a snapshot and the old frame; until the
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
  `AnimTextures` user data, `animated_element`) and show that transformed, only on the outputs
  the transformed rect reaches (`effect_rect`: another scale's texture would be made and drawn
  into whole for nothing; `state_json` counts them, `animation_textures`); `frame.rs`
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
  `after_frames`) prunes finished ones and requests the next frame, as one that may bring nothing
  (`Content::Maybe`: it's for the next vblank, and an output whose redraw found nothing new
  waits for it; asked as a change, each such redraw would run `after_frames` and this again,
  as fast as the loop goes), of the outputs they show on
  (`animation_outputs`: windows' and snapshots' rectangles grown by the shadow and the drop,
  layers' outputs; every output during a workspace slide), taken before pruning so the last
  frame is asked for too; locking drops all.
  Animations are visual only: input sees the real geometry.
- **Window and layer rules** (`rules.rs`, `[[window-rule]]` / `[[layer-rule]]`): match
  windows by app id (`foreign_toplevel::title_and_app_id`: xdg `app_id` or X11
  WM_CLASS class), title and tag (xdg-toplevel-tag, empty if it set none), layer surfaces by namespace, with `*` wildcards
  (`wildcard_match`); all of a rule's matchers must match, matching rules merge in order
  (later wins per field). The result is resolved once and kept in user data
  (`resolve_window_rules` from `place_new_window`, `resolve_layer_rules` from
  `new_layer_surface`), read by `rules::window_props`/`layer_props`, and re-resolved for
  every window and layer surface on a reload that changes the rules
  (`refresh_rule_props` from `apply_config`) and for a window whose title or app id
  changes (`rules_maybe_changed`, from `title_changed`/`app_id_changed` and X11's
  `property_notify`; it lays out again only if the result changed): never matched per frame.
  Window properties: `animations` (all or per kind), and, only when
  it opens: `size` and `open-maximized`/`open-fullscreen` before an xdg toplevel's
  initial configure (`apply_initial_rules` from `handle_xdg_commit`; X11: size by
  `configure`, fill after placing, in `map_window_request`), `position` (from the
  usable area's top-left, title bar included), `open-on-output` (`Pila::rule_output`:
  the output `Pila::output_named` finds, as `Config::output` matches sections, only if it's
  on; it beats a restored session's output, and `Pila::opening_output` gives it to
  `output_geometry_for_window` for fills before mapping and to the initial scale, sent after
  `apply_initial_rules`), `open-focused = false` (mapped and on top, not activated,
  `focus_window` skipped), `open-on-workspace` (`park`ed there,
  unfocused, no animation) and `always-on-top`/`all-workspaces` (`stacking.rs`) in
  `place_new_window`; live, like `animations`: `title-bar =
  false` (`decoration::bar_hidden`: no bar drawn nor laid out, the window stays SSD),
  `corner-radius` (over the global one, in `scene_window`) and `opacity`
  (`SceneWindow::alpha`, passed to every element of the window, title bar and flash
  included; Smithay's elements drop their opaque regions below 1), `blur`,
  `popup-blur` and `popup-corner-radius` (see "Blur") and
  `shadow` (see "Shadows"). Hot paths read them
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
  the output is `winit`, scale only), may be fractional. Every coordinate Pila handles is
  logical; `render.rs` gets the scale per output (`scale::output_scale`). Pila's own
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
  foreign-toplevel identifier), a window at its `window_output`'s scale or, off every output,
  at the fractional scale its client was last told (`window_capture_scale`), so minimizing or
  switching workspace doesn't renegotiate a share's buffers. Sessions live in
  `Pila::capture.sessions`; frames are rendered
  from `scene()` (window sources: only the window, no title bar) straight into the
  client's dma-buf (device and modifiers from `Pila::capture.dmabuf`, set by the backend),
  or offscreen and read back into shm (GL reads rows bottom-up, so they're reversed); the
  offscreen textures are kept by size (up to 4) for the next copies, and dropped from
  `after_frames` once no session, waiting screencopy or screencopy damage tracker is left
  (`Captures::trim_textures`). A
  session's frames are copied in `after_frames` (after a frame drawn, or `capture_due`: a
  commit shown on no output, a capture request), the first one at once (a capture request
  sets `capture_due`), later ones only when the session's own damage tracker sees a change. A copy that fails (at once or on the GPU)
  starts the session over (`Session::start_over`: new tracker, next frame copied at once and
  whole), since its tracker already counted the changes as seen and a still source would
  never send the next frame. Size changes re-send constraints and
  fail the frame; a gone source stops the session (`stop_captures_of`). While locked,
  window captures wait. A window capture's `capture` request sends frame callbacks to the
  window's surfaces shown on no output (covered, or the whole window when it's off the space:
  minimized, parked; `send_capture_frames`), so a client drawing on frame callbacks keeps
  drawing for a share at the capture client's rate, one window frame per captured frame,
  instead of once a second or never; shown surfaces keep the screen's pace. Cursor sessions are created stopped; `paint_cursors` is honored.
  `copy_frame` doesn't wait for the GPU on the event loop: with a native fence to export
  (bare metal, nested; not the headless backend) it returns `Copied::InFlight`, and
  `Pila::when_copied` finishes it (shm: maps the read-back) and sends `ready` from a
  calloop source on the fence fd, unless the client destroyed the frame meanwhile: then
  nothing is written into its buffer, which it may be using again. Without one it waits,
  as before.
  On bare metal an output's captures are filled from the frame its DRM compositor just
  composited, blitted (`RenderFrameResult::blit_frame_result`, between `render_frame` and
  `queue_frame` in `udev/draw.rs`), instead of drawing the scene a second time:
  `Captures::take_composited` picks those that would get the same picture (output not
  turned, since the scanout buffer is in the mode's orientation; cursor painted or not on
  that output; constraints current), with their sessions' damage over the same elements;
  `Pila::finish_composited` delivers them, and `process_captures` skips the sessions checked
  so (`Session::composited`). Copies take what to draw as a `capture::Source`: elements, or
  that blit. Their time counts as captures, not as the output's frame.
  A frame's presentation time (ext's `presentation_time`, screencopy's `ready`) is when its
  content shows (`Pila::presented_at`: `Backend::next_frame_time` of its output, since copies
  follow the frame just queued there; now for a window off every output), which recorders
  use to keep sound and picture in step.
  A dma-buf must have a format and modifier the session offered
  (`dmabuf_offered`: renderable, a subset of what linux-dmabuf imports), else the frame
  fails with `buffer_constraints`, so the client allocates another, rather than `unknown`
  when binding it fails.
- **Screencopy** (`screencopy.rs`, wlr-screencopy v3, for clients that predate
  ext-image-copy-capture): a frame names an output and optionally a region (logical,
  output-local); Pila answers `buffer` (shm Xrgb8888), `linux_dmabuf` when captures may
  use dma-bufs, `buffer_done`. `copy` is rendered from the scene with `capture::copy_frame`
  after the next pass that draws an output or is `capture_due` (it asks for its own output's, as one
  that may bring nothing), so not faster than redraws; `copy_with_damage` waits in `Pila::capture.screencopies` until that
  manager's damage tracker for the output/region (`Captures::screencopy_trackers`) sees a change,
  checked from `after_frames`; a copy that fails drops that tracker, for the same reason as
  `Session::start_over`. A mode/scale change or a removed output fails the frame: a copy
  already waiting when its output goes (`fail_screencopies_of`), or one asked afterwards, at
  once (its output has no frame of its own to come, and no other output's is asked).
- **Portal screen sharing** (`portal.rs`, `screencast.rs`, `share_picker.rs`; prototype, branch only): Pila serves
  xdg-desktop-portal's ScreenCast backend itself, as `org.freedesktop.impl.portal.desktop.pila`
  on the session bus (`Pila::start_portal`, at startup, when `Backend::serves_portal`: bare metal,
  or headless with a bus a test gives it; nested, the host's portal shares the host's screen).
  The bus is served from zbus's thread: `CreateSession` and `SelectSources` answer there, and
  `Start` sends a `portal::Request` to the event loop and waits off it for its `portal::Reply`
  (dropped unsent, `Start` fails). The event loop connects to PipeWire at the first cast (its
  loop's fd a calloop source, iterated without waiting: `dispatch_pipewire`; callbacks only
  queue `CastEvent`s, handled after) and makes a driver stream per session, offering BGRx of the
  output's size at a variable rate up to its refresh; `Start` is answered once the node has an
  id. The offscreen texture its frames are drawn into is made then, drawn into once
  (`Captures::prepare_read_back`), while the app links to the stream: not in the first frame,
  where its cost made the output miss a refresh on bare metal. Captures' textures stay while a
  cast does, and go when the last ends. What's shared is the user's pick: a request waits in `ScreenCasts::waiting` and the picker
  asks for one at a time (below). Frames are made from
  `after_frames` like captures' (`process_screencasts`): the cast's damage tracker over the
  output's elements (and the pointer's, cursor embedded); a change and a free buffer draw it
  offscreen and read it back (`Captures::read_back`, shared with shm captures), written into
  the buffer and queued once the GPU is done (`Pila::when_copied`); with no buffer free the
  tracker starts over, so the next change sends a whole frame. The pointer is hidden, drawn into
  the frames, or (cursor metadata, what browsers ask for, offered by `AvailableCursorModes`;
  they ask for nothing otherwise) written into each buffer's `SPA_META_Cursor`: where its hotspot
  is in the stream's pixels, and its image (BGRA, a named cursor's from the theme,
  `cursor::NamedCursor::image`, a client's from its shm buffer) only when it changed. A buffer
  that only moves the pointer goes out with no picture (`chunk.size` 0): consumers keep the last
  frame. The consumer asks for the metadata too: PipeWire gives a buffer the metas both ends ask
  for. A buffer generation
  (`Cast::buffers`) drops a copy in flight whose buffer went (new format, removed buffers, cast
  ended). A cast ends with its output, a size change (no renegotiation yet), a stream error or
  PipeWire's connection; the app is told (`portal::end_session`: `Closed`), and with no cast
  left the connection goes.
- **Share picker** (`share_picker.rs`): the consent to share. A panel on the output under the
  pointer names the app (`portal::Request::Start`'s `app`: its desktop entry's `Name`, read on
  the bus's thread, else its app id) and shows every output, left to right, as a live preview,
  the one under the pointer selected. Modal: every key goes to it (`input.rs`, before the window
  menu's), every press too (its controls act, the rest does nothing), `Pila::under` hits nothing,
  and every output is dimmed under it (`share_picker::elements`, in front of everything of
  Pila's in `render::output_elements`; never while locked). Arrows/Tab select, Enter/Share
  shares (`Pila::start_cast`), Escape/Cancel answers cancelled; then the next request waiting
  is shown. Opening it ends a switch and closes the cheatsheet, the overview and the window
  menu; locking cancels it and every request waiting (`SessionLockHandler::lock`), the app
  closing its session takes its request away (answered as failed). Its texts are one image
  drawn on a thread with the cheatsheet's code (`shortcuts_view::draw_text`); the panel shows
  without them until then. Each preview is its output's scene (`render::scene_elements`, no
  blur backdrops) drawn small into an `OffscreenWindow` from `after_frames`
  (`Pila::update_share_previews`, with the captures' scene), over a kept background solid:
  damage only where the output changed, and the picker's output is redrawn only then.
- **Toplevel management** (`toplevel_management.rs`, wlr-foreign-toplevel-management v3):
  taskbars act on windows named by their ext-foreign-toplevel-list identifier
  (`foreign_toplevel::window_for_identifier`). Listeners are updated by diffing in
  `refresh_toplevels` (from `after_frames`, after `space.refresh()`): title, app id, states
  (Activated = keyboard focus, Minimized from `minimize.rs`, Maximized/Fullscreen from
  `fill_state`) and the outputs each client's handle was told (a window off the `Space`,
  minimized or on another workspace, reports the outputs it comes back to, from
  `workspaces::space_location`: per-monitor taskbars keep it); new windows get a handle
  per manager, destroyed ones `closed` (`remove_foreign_toplevel`). Requests map onto
  `focus_window`/`restore_window`, `minimize_window`/`show_window`, `set_fill_state(_on)`
  and `close_window`, and are ignored while locked.
- **Gamma** (`gamma_control.rs`, wlr-gamma-control): one control per output, ramps set
  through `Backend::set_gamma` (nested: fails). Destroying the control restores linear.
  udev (`udev/gamma.rs`): a client's ramps wait in `OutputSurface.gamma_pending` (a newer
  change replaces them) until the CRTC is free, then go out as an atomic commit of
  `GAMMA_LUT` alone that doesn't wait, with a page-flip event (`commit_pending_gamma`, first
  thing in `draw::prepare`, before a pass's scene is built): it takes the vblank a frame would (`frame_pending`,
  `gamma_in_flight`), and at its event `frame_finish` presents nothing, leaves `last_vblank`
  (a running animation counts the refresh missed) and draws. Smithay's DRM compositor can't
  carry `GAMMA_LUT` in a frame's commit. The legacy `set_gamma` ioctl, which atomic drivers
  serve with a blocking commit that holds the event loop a whole refresh, is left for
  drivers without `GAMMA_LUT`, a refused commit, and re-applying the ramps remembered in
  `OutputSurface.gamma` after `ActivateSession`, a mode change and a power on (modesets,
  which block anyway). Ramps set while the session is on another VT or the output is off
  are only remembered until then: without DRM master the ioctl fails, and so would the
  client's control (a night light stepping its fade then stops for good). Each logs its time and way (`pila::frames` "gamma", `how`; once in
  the plain log when a client's change goes past 4 ms, `GammaCause::Client`: the ramps put back
  after a modeset block with it, at every startup, and would spend that warning; `scripts/check`
  judges the commits).
- **Output power** (`output_power.rs`, wlr-output-power-management): off = drop the
  `OutputSurface.drm_output` (the CRTC is cleared, the monitor sleeps), on = the CRTC set up
  again with the stored connector and mode through `initialize_with_fallback`
  (`UdevData::set_output_power`; a lighter mode if that one no longer fits, followed by
  `Pila::fit_to_outputs`), with gamma and adaptive sync set again and its recovery and first
  frame reset.
  Code using `drm_output` must handle `None`; a powered-off output stays mapped and counts
  as showing the lock screen. Nested: fails. Clients and the actions go through
  `Pila::set_output_power` (the backend, a lighter mode's refit, power objects told).
  `power-off-monitors` remembers what it turned off (`Outputs::off_until_input`); any input then
  turns those back on (`wake_monitors`, from `process_input_event`), and a key or click that does
  is kept from apps (`suppressed_keys`, `InputState::suppressed_buttons`). Outputs a client turned
  off aren't woken: that client says when.
- **Output management** (`output_management.rs`, wlr-output-management v4): heads are
  `HeadInfo` snapshots (`Backend::output_heads`; nested: the window, transform `Normal`,
  only its scale may change). `refresh_output_heads` diffs against the last snapshot,
  sends the changes and `done` with a new serial, which cancels configurations in
  progress; it runs after connect/disconnect, `apply_outputs` and a nested resize. A
  configuration is checked against the snapshot (`check_output_configuration`: known
  modes only, ≥ 1 head enabled, scale 0.5..8, positions within
  ±`config::MAX_OUTPUT_POSITION`, the bound `[[output]]` is parsed with, so the backends'
  layout math can't overflow) and applied as `[[output]]`-style overrides
  in `Pila::outputs.overrides`, which `Pila::effective_output_config` layers over the file
  for `connector_connected` and `apply_output_config`. If the backend can't do it
  (`apply_outputs` returns false: a failed modeset) the overrides go back to what they were
  and the client gets `failed`. A head's property set twice (a mode and a custom mode
  count as one) is the protocol error `already_set`, as the protocol says. Disabling a head
  (`disable_output`) clears its CRTC, removes its global and runs `output_removed`; the
  `OutputSurface` stays so the head is still listed; `enable_output` is the reverse (also
  used by `connector_connected`). Adaptive sync goes through `DrmCompositor::use_vrr`, after
  checking the connector can do it (a CRTC takes it either way); what `[[output]]` asks is kept
  in `OutputSurface::adaptive_sync` and set again whenever the CRTC is set up again (powered
  on, enabled: `apply_vrr`), as gamma is. While it's on, `frame_finish` reports a variable
  refresh (`Refresh::Variable`) in presentation feedback.
- **Blur** (`blur.rs`, ext-background-effect-v1): a surface's blur region is `Cacheable`
  surface state (`BlurRegion`, committed by Smithay); `Pila::scene` evaluates it per
  layer surface into `SceneLayer::blur` (upper layers only for now) and per window into
  `SceneWindow::blur` (`blur = true` in a rule covers the geometry and title bar, not a
  client's shadow; a window opaque over its geometry and at full opacity gets none,
  `blur::is_opaque`, which a wp-alpha-modifier multiplier below 1 makes not opaque: Smithay's
  surface elements draw the multiplier themselves). Their popups that ask (menus, an input
  method's candidate window) go into `SceneWindow::popup_blur` and `SceneLayer::popup_blur`
  (`render::PopupBlur`: the popup, its place among its root's popups as
  `PopupManager::popups_for_surface` gives them, front to back, its region and its rounded
  corners; the client's region, but a rule's `blur = false` refuses them too; a window rule's
  `popup-blur` decides for its popups alone: `true` blurs behind each one's geometry, or all
  of it without one or for an input method's (`blur::popup_frame`), and only its input region
  there if it set one (GTK's popover is its shape, arrow and rounded corners, in strips, which
  the mask cuts out as it does a client's blur region), that geometry rounded by
  `popup-corner-radius` through `Shaders::up_rounded` as a window's backdrop is, `false` behind
  none; layer surfaces' popups have no such rule). A window or layer surface with
  one has its popups drawn by hand (`render::popup_elements`, as Smithay's `render_elements`
  places them), each with its backdrop right behind it. Before an output's elements are built, `render::output_elements` (which
  the backends and captures use) calls `BlurCache::prepare`: back to front, windows (each
  before its popups, back to front) and then upper layers (likewise), so that each
  backdrop has those behind it, it renders what's behind the region (`render::elements_behind`:
  only the windows and layer surfaces that reach the region; an output's frame likewise leaves
  out the layer surfaces of other outputs; behind a popup, `Behind::WindowPopup` and
  `Behind::LayerPopup`, its own window or layer surface with the popups further back, never
  its flash, drawn as it is even while it animates) into an offscreen texture through a per-region damage tracker, and only when that
  changed runs the dual-Kawase passes (`BLUR_PASSES` levels, custom texture shaders,
  `Transform::Normal` throughout, textures with linear filtering and clamped edges). A
  backdrop's textures are its size rounded up to `BLUR_ALLOC_STEP` (`blur::capacity`), and every
  pass draws and samples only the part in use (`Entry::size`, from the textures' origin; the
  shaders' `tap` clamps samples to its last texels as `CLAMP_TO_EDGE` does at a texture's edge,
  and `Shaders::up_rounded` gets its `target_size` scaled to match): a surface that grows or
  shrinks a few pixels a frame (a panel's animation, a window being resized) keeps them, with a
  new damage tracker for the new size, and only one that outgrows them or shrinks to under half
  of them gets new ones (`blur::fits`); one that outgrew them gets half as much again of room
  where it grew, since it's likely growing on (a panel opening grows by more than the step a
  frame). All
  of it covers the region's bounding box; what's outside the region (clients tessellate
  rounded shapes into strips) is then cleared to transparent (`Entry::mask`, worked out by
  edges so strips leave no gaps at a fractional scale, only when the region changes), and a
  new shape redoes it even with nothing changed behind. Damage only counts within
  `BLUR_REACH` of the region. Only a layer surface's backdrop with no mask is opaque. A change of scale
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
  an opaque one (`SceneWindow::covers`) isn't prepared, but keeps its entry, and so does a window
  off the `Space` (on another workspace, minimized: `Scene::hidden_windows`), so its first frame
  back doesn't allocate and blur everything anew (that missed a refresh on battery, back from
  another workspace); it goes when the window does. Shaders are
  compiled by `render::warm_up`, kept in the EGL context; a failure disables blur.
- **Login session** (`resources/session/`, `sd_notify.rs`): the display manager runs
  `pila-session`. Where `systemd --user` answers and has `pila.service` (installed in
  `SYSTEMDUSERUNITDIR` by `scripts/install`), it runs Pila as that service: it
  refuses a second session while the service is active, imports the login's environment into
  the manager (libseat's logind backend finds the session by `XDG_SESSION_ID` there, the
  service being outside the session's scope; `PATH` and the locale are the services' too) and
  into D-Bus activation, then `systemctl --user --wait start pila.service` in the background,
  waited on so its trap runs: the display manager ends a session by stopping its scope, which
  holds the script and not the service, and the trap stops the service. When the service ends,
  `pila-shutdown.target` (conflicting with them) stops `graphical-session.target` and
  `graphical-session-pre.target`, and `unset-environment` takes back `SESSION_ENV`. The
  service (`Type=notify`, `BindsTo=`/`Before=graphical-session.target`,
  `Before=xdg-desktop-autostart.target`) runs `pila-session --service`, which keeps `pila.log`
  and execs Pila, so Pila is the main process; its stderr reaches the journal as the
  service's (`SyslogIdentifier=pila`). Pila sends `READY=1` (`sd_notify::ready`, once, no
  libsystemd) at the first `run_pending_startup`, after the export below or its timeout, so the
  services of the target find `WAYLAND_DISPLAY`; `session_env` keeps `NOTIFY_SOCKET` from what
  it starts. `PILA_NO_SYSTEMD`, no user manager or no unit: the wrapper runs Pila itself as
  before (and `scripts/tty`, `scripts/check` never use the service). Per-app scopes
  (`app-*.scope`) aren't done.
- **Session environment** (`Pila::spawn`, `start_session`): children get `WAYLAND_DISPLAY`,
  the X display reserved for Pila's Xwayland as `DISPLAY` (from startup, whether X runs yet
  or not; none with `[xwayland] enabled = false`), `XDG_CURRENT_DESKTOP=pila`, `XDG_SESSION_TYPE=wayland`
  and `PILA_SOCKET` (`ipc.rs`); `spawn` adds `PILA_OUTPUT`, the output under the pointer.
  `[environment]` (`Config::environment`: set or removed, the example's then the user's) goes on
  first, so those (`config::PILA_ENV`, which parsing leaves out of it with a warning) and
  `[cursor]`'s `XCURSOR_*` win; Pila's own process environment isn't changed. A reload that
  changes it exports it again (`export_environment`, below).
  Other compositors' IPC sockets and markers (`OTHER_COMPOSITORS_ENV`:
  `SWAYSOCK`, `HYPRLAND_INSTANCE_SIGNATURE`, …) are removed, for a Pila started from inside
  another session: desktop shells pick their compositor backend by them.
  Screen sharing goes through xdg-desktop-portal to Pila's own backend ("Portal screen
  sharing", `resources/portals/pila.portal`), screenshots to xdg-desktop-portal-wlr, as
  `resources/xdg/xdg-desktop-portal/pila-portals.conf` selects; xdpw's own config
  (`resources/xdg/xdg-desktop-portal-wlr/pila`) must be in
  `~/.config/xdg-desktop-portal-wlr/` or `/etc/xdg` (xdpw ignores `XDG_CONFIG_DIRS`;
  `scripts/install` puts both in `/etc/xdg`). `scripts/tty` gives xdg-desktop-portal a
  directory of its own (`XDG_DESKTOP_PORTAL_DIR`: the installed backends, `pila.portal` and
  the `.conf`, which it then reads from there alone). On a TTY, before the startup programs, Pila
  exports that environment to the D-Bus session bus (`export_environment`, `SESSION_ENV`, with
  `PILA_SOCKET` for shells started as services, and the variables `[environment]` sets): on the systemd
  user bus (a login session) also to `systemd --user`, which starts the portals and
  pipewire there; on a private bus (`scripts/tty`, `dbus-run-session`) the
  bus alone, so a test instance leaves the login session's services alone. The export
  is D-Bus itself, no program (`session_bus::export`: `UpdateActivationEnvironment`, then
  systemd's `SetEnvironment`, with the values Pila's programs get, `spawned_env`), on a thread
  (a stuck bus mustn't freeze Pila); the startup programs follow
  when it's done and Pila serves the portal (`startup_step_done`: an app among them could start
  xdg-desktop-portal, which reads a backend's properties as it starts), or after `EXPORT_TIMEOUT`. Nested, apps use the host's portals. Spawned
  programs are kept in `Pila::children` and reaped on SIGCHLD (`reap_children`), each of
  `spawn`'s with its command: one that exits 127, the shell's "command not found", has the
  programs its command line names (`missing_program::programs`: each command's first word, past
  assignments and `exec`/`env`-like words, expanded words left out) looked up in the `PATH`
  its programs get (`[environment]`'s, else Pila's), and each that isn't installed gets a
  notification (`notify_missing_programs`), once a session (`Pila::missing_programs`): a key
  bound to a missing program otherwise does nothing but write to the log. A program that exits
  127 itself, with everything installed, names none. The example's own `autostart` commands
  (`config::example_autostart`, the same words in a file made from it) aren't started at all
  where their programs are missing (`Pila::autostart_installed`, in `main`; `-c` isn't
  filtered): they're the defaults, which a user may leave uninstalled (`pila-desktop` brings
  them), so only the log mentions them. Pila raises
  its soft limit of open files to the hard one at startup (`state::raise_file_limit`: a
  descriptor or more per client, shm pool, dma-buf and fence), and `session_env`'s `pre_exec`
  gives what it starts the limit it was started with, since programs using `select()` break
  past 1024 (Xwayland, started by Smithay, keeps the raised one), and clears the signal mask
  Pila's signal source leaves blocked (a mask survives exec: children would ignore `kill` and
  Ctrl+C); their stdin is null and their stdout goes to Pila's log. The
  signal source is created first thing in `main`, before any thread, so no thread can
  take the signal instead.
- **Activation** (`handlers/xdg_activation.rs`, xdg-activation): a token that activates
  is issued only for a serial newer than the keyboard focus's enter to the client holding it, or
  no older than the last button press (`InputState::last_press`) to the client under the pointer,
  if that press came after the keyboard focus's last change (a clicked notification or panel,
  which take no keyboard; the pointer's enter alone doesn't count, or a window would take the
  keyboard whenever the pointer rests on it); one with no
  serial is issued too, but only marks its window urgent (`UrgentOnlyToken`, see "Urgency"). Tokens
  are single-use and expire after `TOKEN_TIMEOUT`. A request switches to the window's
  workspace (`reveal_workspace_of`), shows it if minimized and focuses it if it's mapped;
  while locked it does nothing (a token from before the lock), but an urgency token still marks.
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
  Pila on the same state (`take_sessions_lock`, a `scripts/tty` next to the login session),
  only in memory (`load_and_persist`). What a client can leave there is bounded
  (`MAX_TOPLEVELS`, `MAX_NAME_BYTES`, `MAX_CLIENT_SESSIONS`).
  Not `is_privileged`: it only touches the client's own windows. X11 windows and clients
  that don't use it open centered.
- **Foreign toplevels** (`foreign_toplevel.rs`, ext-foreign-toplevel-list, read-only): a
  window's handle is created in `place_new_window`, kept in its user data, updated on
  commit (X11: `property_notify`) and closed in `forget_window` (a toplevel destroyed, an X11
  window unmapped, Xwayland gone).
- **Session lock** (`session_lock.rs`, ext-session-lock): guaranteed where input reaches
  clients, not by each path. While `is_locked()`, `scene()` holds only lock surfaces (on
  black); `Pila::under` gives only `Under::Lock` (and so `surface_under`); and the
  focus types refuse everything that isn't a lock surface (`Pila::is_lock_surface`: one or a
  subsurface of one): `KeyboardTarget for KeyboardFocus` (enter, keys, modifiers),
  `PointerTarget`/`DndFocus for PointerFocus` (enter, motion, buttons, scrolls, gestures, drags)
  and `set_keyboard_focus`, through `refused_while_locked`, which logs the attempt once per lock
  with a backtrace and panics in debug builds (the tests' builds): any path that gets there is
  a bug. Every input reaches clients through those types, whatever sent it (Pila or a Smithay
  grab), but for an input method's keyboard grab, which sends keys to the input method's own
  object: while locked, keys bypass it, and a lock surface never gets the text input (see
  "Input methods"); leaving is always allowed. Above that, each path still refuses what it shouldn't do
  while locked (`focus_window`/`focus_layer` keep the keyboard on a lock surface, popup and
  move/resize grabs are refused, bindings, gestures and `pila msg` don't run their actions,
  nor does `run_action` itself, save `runs_while_locked`, a VT switch and Quit in debug
  builds). `lock` ends grabs before the state changes (ending one sends its focus a motion).
  `tests/lock.rs::nothing_but_the_lock_gets_input_or_shows_while_locked` tries every action
  (`Action::examples`, a table a new kind of action doesn't build without), gesture,
  activation, taskbar and pager request and the window menu while locked. The lock is confirmed (`confirm_lock`)
  once every output that's on shows a full locked frame (bare metal: at its vblank,
  `maybe_confirm_lock`; nested: submitted; headless: drawn), at once with none on. `unlock` unlocks at once: Smithay
  calls it only for the confirmed lock's `unlock_and_destroy`, any other being a protocol
  error. A dead locker leaves the session locked;
  a new locker may take over.
- **Idle** (`idle.rs`, ext-idle-notify + idle-inhibit): every input event sets
  `Pila::input_activity`, which `after_frames` turns into one `notify_activity` per loop
  iteration (each re-arms every notification's timer: not a thousand times a second for a
  1000 Hz mouse); Smithay runs the notification timers on `loop_handle`. Idle inhibitors
  only count while their surface is part of a mapped window or layer surface and the
  session isn't locked; `update_idle_inhibit` re-checks in `after_frames` (after a frame,
  a client dispatch or input). A new input path
  must go through `process_input_event` (or set `input_activity`) to count as activity.
  Pila has no idle action of its own: locking and turning the monitors off are swayidle's
  (the example's `autostart`), through `power-off-monitors` (`power_off_monitors`: the next
  input wakes them, swallowed, `wake_monitors`). An old file's `[idle] monitors-off` is read
  for a warning only (`IdleFile`).
- **Pointer constraints** (`pointer_constraints.rs`): a lock/confine activates only while
  the pointer is over its surface (inside its region) and that window has keyboard focus;
  `SeatHandler::focus_changed` releases it (`release_constraint_for_focus`), which makes
  Mod+Tab / clicking elsewhere / locking the escape. The motion path checks the active
  constraint before moving the pointer. Nested, relative motion is derived from the host
  pointer (`Pila::input.host_pointer`); a DRM backend sends real relative motion through the same
  checks. A locked client's cursor position hint (`cursor_position_hint`) moves Pila's pointer
  there, kept within its window, so unlocking doesn't jump.
- **Pointer warp** (`pointer_warp.rs`, wp-pointer-warp-v1): a client moves the pointer within
  its own surface. Refused while locked, with a serial other than the pointer's latest enter,
  unless the pointer is on that surface or a surface of its tree, and unless the place it asks
  for is on that tree too (`surface_under`: never onto another client's surface or nothing).
  The move is `Pila::warp_pointer`'s (a binding's too): a constraint holding the pointer keeps
  it, clients get it as motion, and it isn't activity.
- **X11** (`xwayland.rs`, `focus.rs`): Xwayland on demand. At launch Pila only reserves an
  X display (`reserve_xwayland`: Smithay's `XWaylandDisplay`, its number, lock file and
  listening sockets, held until exit: the socket file only, never an abstract socket, which has
  no permissions) and its cookie (`xauth::Xauthority`: one MIT-MAGIC-COOKIE-1, random, in
  `$XDG_RUNTIME_DIR/pila-xauthority-<N>`, mode 0600, a `FamilyWild` entry as Flatpak writes them;
  removed with the reservation, in `stop_xwayland`), unless Xwayland isn't installed or `[xwayland] enabled =
  false`; Xwayland runs with `-auth` on that file, so a client without the cookie is refused (the
  window manager's `-wm` connection needs none, and Pila's own XRes connection,
  `only_pilas_x11_clients`, passes it through `xauth::connect`, as Pila's environment has no
  `XAUTHORITY`); spawned clients get its `DISPLAY` and `XAUTHORITY` from the start (never the
  host's; `Pila::x11_display`, `Pila::x11_authority`, none while turned off), and the startup programs don't wait for X. Turned on or off on reload
  (`xwayland_config_changed`): on, the display is reserved then; off, it's let go at once if
  Xwayland doesn't run, else when it stops (idle or crashed: `listen_for_x11` gives it up
  instead of watching again), so its apps keep running meanwhile.
  While Xwayland doesn't run, Pila watches the sockets (`listen_for_x11`) without accepting:
  the first X11 app to connect makes one readable, and Pila stops watching (from an idle
  callback, `x11_app_connecting`: a calloop source removed from its own callback stays in the
  poll) and starts Xwayland on the display (`spawn_xwayland`, inheriting Pila's environment),
  which takes the connection waiting in the queue. Once it's ready its `X11Wm` starts and gets
  the Wayland selections set meanwhile (`XwaylandState::clipboard`/`primary`, kept by
  `wayland_selection_changed`). Dying before it's ready, which Smithay's event source misses
  (it takes the closed display pipe for "not yet"), is caught by `XWaylandSource`
  (`xwayland_failed`). When it dies later (`XwmHandler::disconnected`, `xwayland_gone`) its
  windows and selections go and Pila watches the sockets again, so the next app starts it on
  the same display; a failure of either kind counts against `MAX_RESTARTS` in `RESTART_WINDOW`
  (`may_restart`: a queued app would start a broken Xwayland forever), past which Pila lets the
  display go (`stop_xwayland`) and apps are refused. With `[xwayland] idle-timeout`, the last
  X11 window going (`unmapped_window`; or Xwayland ready with none) arms a timer
  (`arm_xwayland_idle_check`); when it fires with still no X11 window, a thread asks X through
  XRes for its clients' processes (`only_pilas_x11_clients`): if all are Pila's (its window
  manager, the asking connection) or Xwayland's own, Pila stops the server alone
  (`stopped_idle`, the event source dropped and a SIGTERM, since a dropped Wayland client is
  only cleaned up at the next dispatch) and lets the window manager go when its connection
  closes, through `xwayland_gone`, as X may still send it events; otherwise (a settings
  daemon, an app with no window yet) it checks again a timeout later. Smithay's `X11Wm`
  removes its own event sources when dropped (Pila's fork), so cycles leak nothing. X11 windows are `Window::new_x11_window` in the same
  `Space`, so code that handles windows must not assume `window.toplevel()` is `Some`: use
  `wl_surface()`, `fill_state`, `close_window`, `focus_target`, and branch on
  `x11_surface()` where X11 differs. A window may get its `wl_surface` after it's mapped,
  even after that surface's commit (X names it last): `surface_associated` redraws everything
  then, as what it shows appears, and the focus shown moves to it. Keyboard focus is `focus::KeyboardFocus` (Wayland
  surface or X11 window, so Smithay sets the X input focus); compare surfaces through
  `Pila::keyboard_focus()`. Whenever Pila moves a managed X11 window it must tell X
  (`x11_window_moved`); focusing raises it in X too. Where X has a window, and the size it
  was configured to, is `X11Surface::last_configure()`; `geometry()` is only its visible
  area within its buffer (minus `_GTK_FRAME_EXTENTS`). A window that maps hidden (Smithay
  turns `WM_HINTS` IconicState into `_NET_WM_STATE_HIDDEN`) is minimized, not focused, by
  `place_new_window_with`. Override-redirect windows (menus, tooltips) go where X puts them, unfocused, except one
  with no menu-like window type that takes input (`override_redirect_takes_keyboard`, as in
  wlroots): a launcher like dmenu grabs the keyboard in X, which only gets keys while an
  Xwayland surface has the Wayland keyboard, so it gets the keyboard (not over the lock screen
  or an exclusive layer surface, and out of the focus history) and `unmapped_window` gives it
  back. X11 maximize/fullscreen go through `set_fill_state`
  (`_NET_WM_STATE` + a direct configure); resize grabs configure X11 windows per motion.
  Selections are bridged both ways (`SelectionHandler` in `handlers/mod.rs`, XwmHandler
  selection methods); only a focused X11 app may read. The idle check's timer and the wait
  for it are housekeeping, not frame pacing: they exist only with `idle-timeout` set, and
  wake Pila once per timeout while Xwayland runs with no window.
- **Snapping and keyboard moves** (`snap.rs`): a snapped window fills half of its output's
  usable area (less `gaps`, from the edges and between halves: `snap::side_rect`; a maximized
  window fills `snap::gapped`, the area less `gaps` all around, through `set_fill_state_on`,
  `place_new_window` and the drag's preview, `snap_target_rect`; with `gaps` above 0 it keeps
  a floating window's shadow and rounded corners, `scene_window`'s `fills_output`; a change of
  `gaps` refits like a change of the area), gets xdg `Tiled*` states, and keeps its pre-snap rect in `SnapState`.
  Snapping is floating geometry, not a fill state (but a change of the usable area refits it,
  as `refit_to_usable_area` does filled windows): maximizing a snapped window
  (`set_fill_state_on`) or resizing it (`begin_resize`, keyboard resizes) calls
  `forget_snap`, and maximizing keeps the pre-snap rect as the restore geometry.
  `MoveSurfaceGrab` previews the edge snap target (`Pila::snap_preview`, a solid element
  drawn just above the windows) and applies it on release; dragging a snapped or filled
  window first restores its old size under the cursor, once the pointer has moved
  `RESTORE_DISTANCE` (`restore_for_drag`; a click on its title bar or a Mod+click isn't a
  drag and leaves it as it is); a snapped one also goes down as far as the pointer did since
  the press, which a client's own drag (GTK's, after its own threshold) makes sizable. Keyboard
  move/resize/center go through `apply_rect`, which also configures X11 windows.
  `Side` also names the top and bottom halves (`snap-top`, `snap-bottom`, from the keyboard
  only) and the four quarters (`snap-top-left`, ... or a drag near a corner: `edge_target`);
  each is tiled on the edges it touches. `snap <direction>` (the default Mod+arrows,
  `Pila::snap_toward`) takes one step from where the window is (floating, maximized,
  fullscreen or snapped: `Placement`) on the grid of halves and quarters, the table being the
  pure `snap::step`; its result goes through `snap_window`, `set_fill_state` or
  `restore_window_size`, so it needs no geometry of its own.
- **Window menu** (`window_menu.rs`): opened by a right press on Pila's title bar
  (`on_decoration_press`, at the pointer rounded down, so the pointer is inside it) or by
  `show_window_menu` (checked like a move request, `check_grab`, or with the serial of that
  client's last press within a second, `recent_press`: GTK asks after the release of a quick
  click; its position is surface-local). It's `Pila::window_menu` plus a pointer grab
  (`WindowMenuGrab`): no client gets the pointer while it's open. The press that opened it,
  known by its serial, reaches the grab right after and counts for nothing; the release of
  that click runs nothing within `OPENING_CLICK` (the menu opens under the pointer); any other
  release over a row runs it. A press outside closes it, and a right one is then replayed
  (`pointer_button` from an idle callback: another title bar's menu, an app's own). Inside
  the grab the pointer is locked, so the grab ends itself through its `handle` and runs an
  item from an idle callback (with the `wake` ping); `close_window_menu` (keys,
  `forget_window`, a lock, a workspace switch) ends it through the pointer. The keyboard
  filter gives it every press while open (kept as `menu_key` for after the filter, in
  `on_keyboard`). Drawn first by
  `output_elements` from `Scene::window_menu`, its label textures and solid buffers kept in
  `MenuBuffers`.
- **Urgency** (`urgency.rs`): a mark in a window's user data (`set_urgent`, `is_urgent`), set by
  an activation whose token had no serial (`UrgentOnlyToken`, accepted for any client in
  `token_created`) or X11's `WM_HINTS` urgency hint (`property_notify` `Hints`), never
  on the focused window, and cleared when it gets the keyboard (`SeatHandler::focus_changed`,
  from the surface it's given: Smithay holds the keyboard's lock there, so asking it for the
  focus deadlocks). Shown by the title bar (`colors.bar_urgent`), ext-workspace's `urgent`
  state (`workspace_states`, one state per workspace per manager) and `pila msg`.
- **Always on top, all workspaces** (`stacking.rs`): two marks in a window's user data, from a
  rule when it opens or a binding, shared by its transient family (`mark_new_window`,
  and again in `transient_parent_changed`). `keep_on_top` puts the marked ones back above the others in their order, doing
  nothing if they already are (a change of order means damage). **Whatever raises a window has
  to end in it:** `raise_family` and `keep_transients_above` do, and so every
  `map_window` (it raises, whatever its `activate`) is followed by one of them, but X11
  override-redirect windows' (on top by design) and `unpark_active`'s (`show_workspace` ends in
  `keep_on_top`). `top_layer_covered` skips a marked window
  that isn't fullscreen itself, so one kept over a fullscreen window doesn't bring panels back.
  A window on all workspaces is never parked: `show_workspace` leaves it in the `Space`, raises
  it after focusing, and a slide draws it fixed (`SceneSlide::carried`); `workspace_of` says the
  active one; `move-to-workspace` removes the mark. Neither is saved with a session.
- **Between outputs** (`stacking.rs`): `move_focused_to_output` keeps the window's place relative
  to the usable area (`fit_in_area`), fills the new output if it was filled, or takes the same
  side (`move_snapped_to`), and translates where it goes back to; the move animates
  (`animate_geometry`, see "Animations"). `focus_output` moves the pointer
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
  asked. `action`, `focus`, `close` and `minimize` are refused while locked, but for the actions that get
  past nothing (`Action::runs_while_locked`: the monitors' power, as idling locks and then turns
  them off, and `switch-layout`, for a lock screen's password). An action goes through
  `Action::parse` and `run_action`, with a window switch ended at once, as no key is held.
  `subscribe` adds the client (a `Weak`) to `Pila::ipc_events` with the kinds it wants and the
  last event of each it got; its first events go right after the answer (`Client::after_answer`).
  `Pila::send_ipc_events`, in `after_frames` right after the taskbars' `refresh_toplevels`, builds
  each kind someone wants (only then), and sends it to those whose last differs: whole lists,
  the window in a pointer move or resize (`grabbed_window`) with the geometry it had in the last
  windows event (`Subscribers::settled`), so that a drag sends one event, after the release (an
  input event, which redraws). A change that may come with no frame
  (`set_output_power`: with every monitor off nothing is drawn) marks them due
  (`Pila::ipc_events_due`), and `after_frames` sends them even when it draws nothing. The keyboard layouts are checked
  every loop iteration instead (`send_keyboard_events`, before `after_frames` returns early): an
  xkb option can switch them with nothing to draw. Only against those the last event was made
  from (`keyboard_layouts_are`, nothing built): the event is made only when they changed. The
  colors (`[colors]`, for clients that look like Pila) are sent when `apply_config` changes them
  (`send_colors_event`), the window switch in the iteration it changed
  (`send_switch_events`, see "Window switching"); `ipc_colors` writes them back as the config does (`hex`, from
  premultiplied). `keyboard_layout.rs` reads them from the
  seat's xkb state and `switch-layout` sets them; while a virtual keyboard's keymap is in place
  it's the physical keyboard's that count, the names cached when that keymap swapped in
  (`InputState::layout_names`) and the layout it comes back to (`saved_layout`), which a switch
  changes instead.
- **Maximize/fullscreen** state lives in the toplevel's *pending* xdg state (`fill_state`
  reads it). `set_fill_state` is the single entry point; it stores the pre-fill geometry in
  the window's user data (`RestoreGeometry`: its place and the size it was last asked for,
  pending, not the one it has drawn, which may still be a filled one) and restores it on exit; a window that
  started filled (no saved geometry) is restored to two thirds of its output, centered.
  A client's request (`update_fill`) and a taskbar's (`toplevel_management.rs`) go through
  `request_fill_state`, which does nothing when asked for the state the window is in (GTK 4
  unsets fullscreen as a drag on its own title bar starts: `set_fill_state` would place a
  window that isn't filled); a client still gets its configure. Fullscreen on a given
  output still goes to `set_fill_state_on`, since that may move it to another output.
- **Decorations** (`decoration.rs`): Pila prefers server-side and honors client requests
  (`handlers/xdg_shell.rs`); clients that don't bind xdg-decoration (GTK, weston) draw
  their own. A decorated window's `Space` location is still its geometry origin; the title
  bar (`TITLE_BAR_HEIGHT`) sits above it and an invisible resize border surrounds both.
  Hit testing goes through `Pila::under`, which for a window returns a `Hit` (client
  surface or `Decoration`) front to back; `surface_under` (pointer focus) ignores
  decorations. Layout (placement, maximize) uses the *pending* decoration mode via
  `layout_offset`; drawing and hit testing use the *committed* state (`has_title_bar`).
  Bar textures (buttons, title) are per-window `MemoryRenderBuffer`s cached by a key and
  re-rendered only when it changes, one set per buffer scale (`TitleBarBuffers::scaled`, the
  last few used: drawing a window on an output of another scale leaves the others' alone); they can't be shared between windows (element ids must
  be unique per frame). A window keeps a few looks of each (`variant`: a button focused,
  unfocused and hovered, the title focused and unfocused), so focus going back and forth
  (Mod+Tab) rasterizes nothing again. A title that fit whole is kept for any room it still fits in
  (`TitleTexture::shows`): a resize changes the room every frame, and it would be shaped and
  uploaded again each time. Titles come from `text.rs` (cosmic-text), in the system's sans-serif
  at `text::FONT_SIZE`: fonts load on a
  background thread at startup, which makes sans-serif the family `fc-match sans-serif` names
  (`text::follow_fontconfig`; cosmic-text's own is Open Sans, installed or not), the one the
  tools and GTK draw in, and until then `render_title` returns `None` and bars draw
  without text; once loaded (and warmed up: common characters shaped and rasterized on that
  thread, `text::init`) a ping asks for a redraw (`main.rs`). With no fonts at all (CI's system,
  or `PILA_HEADLESS_NO_FONTS=1` for the tests) `text::unavailable` is set and the ping asks for
  nothing but to hide a shortcuts panel shown meanwhile, which isn't shown then: it's all text.
  The title texture is at most `MAX_TITLE_WIDTH` (8192 px) wide, however wide
  the window, and `render_with` returns `None` on sizes that overflow.
- **Rounded corners** (`rounded.rs`, `corner-radius`, default 8, and `rounded-corners`, `"top"`
  by default as GTK rounds its own windows, or `"all"`: `rounded::Corners`, whose `radii` gives
  each corner's radius, top-left first, to every shader through a vec4): `Pila::scene` gives each
  window a `Rounding` (geometry plus title bar; none when maximized or fullscreen; the global
  radius not for a client that draws its own shadow, `bbox` past its geometry, as for shadows,
  since it draws its own corners: a rule's radius still applies; a blur behind it still gets
  the configured radius, on all four corners whatever `rounded-corners` says (such an app,
  libadwaita's, rounds them all), `SceneWindow::backdrop_rounding`, as the corners it leaves
  clear would show a square backdrop; radius
  at most half the shorter side and, with a bar, its height). `render::single_window_elements`
  then builds the window's elements itself (`rounded_window_elements`: popups first and unclipped, as
  `Window::render_elements` does, and an X11 window's own opacity, `render::own_opacity`, as
  Smithay's `X11Surface::render_elements` does) and wraps its own surface elements that reach into a
  corner in `RoundedSurface`, whose `draw` overrides the default texture program with one
  that fades out the corner (`input_to_geo` inverts Smithay's `build_texture_mat`, so
  keep it in step with Smithay). Elements away from the corners stay plain (direct
  scanout). Content outside the geometry (CSD shadows) is not clipped. Opaque regions
  lose the corner squares. The bar background, bell flash and snap preview are
  `RoundedSolid` (pixel shader over a `SolidColorBuffer`'s element; the bar always its top
  corners, the flash and the preview the window's); the close button is
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
  `Pila::finish_resize_commit` (called from `CompositorHandler::commit`) repositions the
  window when resizing from the top/left edges once the client commits the new size.
