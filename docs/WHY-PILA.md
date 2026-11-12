# What sets Pila apart

Pila is a stacking Wayland compositor in Rust, on Smithay's main branch, made to host
layer-shell desktop shells such as [Noctalia](https://noctalia.dev) and Dux Shell. This page
covers its design choices and how it differs from other compositors. Features are in the
[README](../README.md#features), usage in the [manual](MANUAL.md), and each flow and invariant
in [ARCHITECTURE.md](ARCHITECTURE.md).

## Where it fits

- **Stacking, with server-side decorations:** overlapping windows, xdg-decoration with Pila's
  own title bars and resize borders (client-side ones honored), snapping, workspaces spanning
  all outputs. Shells built on layer shell usually run on tiling compositors (niri, Hyprland,
  sway); Pila gives them a floating desktop.
- **A compositor, not a desktop environment:** bar, launcher, notifications, wallpaper and lock
  screen are external clients (layer shell, ext-session-lock, ...). The floating modes of KDE
  Plasma, GNOME and COSMIC come with their own shell; labwc keeps Openbox's model and XML
  config; Wayfire is built around Compiz-style plugins. Pila has one TOML file and an IPC
  socket.
- **Checked against the shells it targets:** `scripts/shells` runs Noctalia and Dux Shell on a
  headless, sealed-off Pila and drives their IPC through every feature that depends on the
  compositor (ext-workspace, foreign toplevels, gamma, screencopy, session lock, output
  management, ...), after any change to those protocols, any Smithay bump and before each
  release.

## Performance model

One invariant: **Pila never makes an output miss a refresh while there is something new to
show** (an animation, a drag, a client's frame), at the output's own rate, with a CPU budget of
4 ms per frame and output (release build, integrated AMD GPU). And it draws nothing when
nothing changed.

- **Per-output, on-demand redraws:** every change asks for a redraw of the outputs it touches
  (a window's visual rect, a surface's outputs before and after a commit, the pointer's old and
  new output); nothing is redrawn by a timer. Each output's element list leaves out windows
  that don't intersect it. In the test suite, an output whose content changed without a redraw
  being requested is a panic, so a missing request can't slip in.
- **Damage tracking everywhere:** per output, per capture session and per blurred region;
  nested, buffer age for partial repaints.
- **Idle-redraw throttling:** a redraw that finds nothing new (a client committing without new
  content in a loop of frame callbacks or FIFO barriers, the pointer moving with no cursor
  drawn) makes the next one on that output wait until half a period before its vblank. A
  client can't make Pila redraw faster than the refresh rate; Pila's own changes never wait.
- **Hidden surfaces throttled:** frame callbacks go only to surfaces on the output being
  drawn; surfaces shown nowhere get one per second, so a covered client doesn't spin.
- **Blur proportional to change:** dual-Kawase blur (ext-background-effect, or per window by
  rule) renders only the windows and layer surfaces that reach the region, through a damage
  tracker of its own, and reruns the passes only when what's behind changed. A window covered
  by an opaque one isn't processed.
- **Nothing blocking on the event loop:** font loading and shaping, the cursor theme, D-Bus
  environment export and GPU waits for captures run on threads or behind fences. One-time
  costs (shader compilation, offscreen textures for workspace slides, common glyphs) are paid at
  startup, not in the first frame that needs them. Title bars are cached textures that survive
  resizes and focus changes.
- **Direct scanout:** on the DRM backend, a frame that is a single client buffer (a fullscreen
  game or video, an unobscured surface covering the output) goes on the primary plane with
  nothing composited. Such surfaces get per-surface dmabuf feedback with a scanout tranche of the
  plane's formats, so the client allocates buffers the plane accepts. On by default, on the
  primary GPU's outputs; `direct-scanout = false` turns it off.
- **Measured, not assumed:** `RUST_LOG=pila::frames=debug` logs, per output every 5 s, frame
  count, mean and worst CPU time, missed refreshes, idle redraws and direct-scanout frames.
  On bare metal, missed refreshes are counted from the DRM vblank sequence, and each late
  frame is attributed: held back by the idle wait, drawn late, or shown late after being
  submitted in time. Captures and the event loop's own housekeeping are logged apart.
  `scripts/check` runs real clients through scenarios on a TTY and judges these numbers;
  changes to rendering, input or the event loop are checked with it before they're committed.

## Client pacing and buffers

- **Slow clients delay only themselves:** a commit with a dmabuf the client's GPU is still
  writing is held (a pre-commit blocker on its implicit fence, or the linux-drm-syncobj acquire
  point) and applies when it signals; Pila never waits on it to build a frame.
- **wp-fifo:** barriers are signaled by the frame that shows the surface, so a FIFO client
  gets one content update per refresh; barriers of hidden surfaces or surfaces on an output
  that's off are released once a second.
- **wp-commit-timing:** a timestamped commit is applied for the frame that will be presented
  at that time, computed from the kernel's vblank timestamps.
- **Presentation feedback** with the kernel's timestamps, CLOCK_MONOTONIC throughout.
- **Bounded geometry:** every client-supplied size and offset (buffers, subsurface positions,
  viewport destinations, window geometry, layer-shell sizes, margins and exclusive zones, popup
  positioners) is checked against a 32768 logical-pixel limit before any arithmetic, so no
  client can overflow Pila's or Smithay's layout code; violations are protocol errors.

## Screen capture

- **ext-image-copy-capture** (implemented in Pila on the `wayland-protocols` bindings, output
  and toplevel sources) and wlr-screencopy for older clients such as xdg-desktop-portal-wlr.
- **No second composite on bare metal:** on the primary GPU's outputs, an output capture is a
  GPU blit of the frame the DRM compositor just composited for that output, with the session's damage over the same
  elements.
- **Zero copy, asynchronous:** frames are rendered straight into the client's dmabuf, only when
  the session's damage tracker sees a change; completion is signaled from a native fence fd on
  the event loop, never by blocking on the GPU. Shm falls back to read-back.
- **A/V sync:** each frame's presentation time is when its content is scanned out (the
  output's next vblank), not when it was copied.
- **Hidden toplevels stay live:** a captured window that is covered, minimized or on another
  workspace receives frame callbacks at the capture client's rate, so a window share doesn't
  freeze at one frame per second.

## DRM/KMS backend

- **Per-output failure isolation:** a frame that fails at the DRM level (render, commit, no
  DRM master after a VT switch) blocks only that output, retried with exponential backoff; the
  others keep running.
- **Mode fallback:** a mode the link can't carry falls back to the same size at a lower
  refresh rate, then smaller sizes, tested with atomic test commits.
- **DisplayPort link retraining:** outputs whose `link-status` the kernel set to bad (after a
  resume, through a dock) are retrained, which Smithay doesn't handle.
- **Safe hotplug:** a removed output's `wl_output` global is disabled first and destroyed later,
  so clients binding it as it disappears don't hit a protocol error.
- **Lock confirmed on scanout:** ext-session-lock is confirmed only once every enabled output
  has shown a locked frame at its vblank, so no unlocked frame can appear after the locker is
  told it's locked.
- **Multi-GPU** (`other-gpus = true`, experimental): outputs on a secondary GPU are rendered by
  the primary (Smithay's `MultiRenderer`) and copied through a dmabuf it imports, or through
  system memory; software renderers (llvmpipe on a KMS device) are detected and kept off the
  dmabuf path. See [MULTI_GPU.md](MULTI_GPU.md).
- **Gamma without stalls:** wlr-gamma-control ramps go out as a non-blocking atomic commit of
  `GAMMA_LUT` alone, taking a vblank like a frame, instead of the legacy ioctl, which atomic
  drivers serve with a blocking commit (a night light's fade would stutter).
- Adaptive sync per output; output power (wlr-output-power-management) by clearing and
  restoring the CRTC.

## Protocols

Standard `wayland-protocols` first (stable, staging, then unstable where nothing newer
exists), implemented in Pila when Smithay lacks them. wlroots protocols only where shells need
them and no `ext-` replacement exists yet (layer-shell, screencopy, foreign-toplevel-management,
gamma-control, output-management, output-power-management, virtual-keyboard), each to be
dropped when one lands. No KDE, GNOME or Hyprland protocols.

Among the less common ones: xdg-session-management (window placement and state restored across
client and compositor restarts), ext-background-effect, ext-workspace, ext-image-copy-capture,
ext-data-control, xdg-dialog, xdg-toplevel-tag, xdg-system-bell, wp-fifo, wp-commit-timing,
linux-drm-syncobj, wp-security-context, cursor-shape, pointer-constraints and
relative-pointer, idle-notify and idle-inhibit. Xwayland runs through Smithay's X11 window
manager, restarted if it dies, with selections bridged both ways.

## Security model

- **Privileged globals filtered by security context:** clients connected through
  wp-security-context (Flatpak) can't see screen capture (both protocols), data-control,
  layer shell, session lock, foreign-toplevel list and management, gamma, output management and
  power, ext-workspace, virtual keyboard, keyboard-shortcuts-inhibit, idle-notify or
  security-context itself. Any new global that exposes or controls other clients, input or
  outputs is filtered too, by policy.
- **Session lock enforced at the focus types:** while locked, the keyboard and pointer focus
  types themselves refuse any target but a lock surface, so input can't reach another client
  through any path, Pila's or a Smithay grab's; a path that tries is a panic in debug builds.
  Hit testing and the scene contain only lock surfaces, and a test runs every action, gesture,
  activation and taskbar request while locked.
- **xdg-activation tokens** only for a serial newer than the client's keyboard focus, or its
  pointer focus (a clicked notification), single-use and short-lived; one from before a lock
  activates nothing.

## Configuration and IPC

- **TOML, hot-reloaded** via inotify on the config's directory (symlinked dotfiles included).
  An invalid file keeps the running configuration and raises a notification with the line and
  the error.
- **`pila --check-config`** validates a file as Pila reads it, compiling the XKB keymap too,
  and exits 0 or 1: usable in a dotfiles CI or a pre-commit hook.
- **Window and layer rules** matched on app id, title or xdg-toplevel-tag (layer namespace for
  panels), re-evaluated on title changes and reloads: workspace, geometry, maximized or
  fullscreen, always on top, sticky, decorations, corner radius, opacity, blur, shadow,
  animations.
- **IPC:** a Unix socket served on the event loop; `pila msg` queries windows, outputs,
  workspaces and keyboard layouts as text or JSON, runs any action a binding can, and
  `subscribe` streams events. The format is documented in [IPC.md](IPC.md).

## Testing and reliability

- **Integration tests on a headless backend:** each test starts a real Pila on virtual outputs
  (GPU rendering, or llvmpipe in CI) and drives it with Wayland clients and injected input
  through a control socket; no fixed sleeps, only polled conditions.
- **A feature inventory that can't rot:** [`tests/FEATURES.md`](../tests/FEATURES.md) maps
  every feature and invariant to its tests, and a test checks that every test named there exists
  and every integration test is named there. Other tests check that the docs name only code that
  exists and that the manual and man page list every default binding and flag.
- **Regression tests first:** a bug fix starts with a test that reproduces it.
- **Fuzzing** (cargo-fuzz): the config, IPC request, EDID and gamma-ramp parsers, and arbitrary
  protocol request sequences against a running Pila.
- **The DRM backend in CI:** a workflow runs it on the kernel's vkms driver: frames, gamma,
  output hotplug, and a second vkms device as a second GPU added and removed.
- **No panics past startup** except on stated local invariants; poisoned mutexes and missing
  user data are handled, and a crash logs its backtrace.

## Portability

No assumptions about distro, init system or display manager: logind, elogind or seatd for the
session; optional tools (`systemd-cat`, `logger`, `dbus-run-session`, a per-user bus) are
detected, and the install paths are configurable. Packages for Arch (AUR), Debian, Ubuntu,
Fedora and Nix; CI builds on each of them, and apart with the oldest supported Rust
(`rust-version`).

## Desktop details

Tap Mod alone for the launcher, Alt+Tab and Mod+Tab by recency, three- and four-finger swipes,
shake to find the pointer (named cursors drawn from the theme's larger sizes), urgency shown as
an amber title bar, a visual bell, focus returned to the previous window, short animations that
can be turned off globally or per app.

## Not there yet

Pila is alpha. Missing: touchscreens, tablets (tablet-v2) and input methods (text-input,
input-method). NVIDIA is untested; secondary-GPU outputs are off by default and untested on
real hardware. See [Status](../README.md#status) and, if you can, [help test
it](../README.md#help-test-pila).
