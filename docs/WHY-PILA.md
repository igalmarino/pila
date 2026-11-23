# What sets Pila apart

Pila is a stacking Wayland compositor in Rust, on Smithay's main branch, made to host
layer-shell desktops: standard tools for the bar, launcher and notifications (waybar,
fuzzel and mako by default), or a full desktop shell. This page
lists its features, then covers its design choices and how it differs from other compositors.
Usage is in the [manual](MANUAL.md), and each flow and invariant in
[ARCHITECTURE.md](ARCHITECTURE.md).

## Features

- **Windows:** server-side title bars (minimize, maximize, close) and resize borders, or
  client-side decorations for apps that draw their own; Mod+drag to move and resize;
  maximize and fullscreen; snapping to screen halves and quarters by keyboard or by dragging to
  an edge or corner; moving, resizing and centering from the keyboard; minimize; showing the
  desktop; keeping a window above the others or on all workspaces; moving windows and focus
  between monitors; switching by recent use (Mod+Tab, Alt+Tab), the windows shown as live
  previews while it's held; tapping Mod alone opens the
  launcher (fuzzel by default); workspaces spanning all monitors (Mod+1..N, Mod and the mouse wheel,
  four-finger swipes); a window menu on the title bar (right click); windows asking for
  attention marked with an amber title bar; volume, brightness, media and screenshot keys bound
  by default (wpctl, brightnessctl, playerctl, grim and slurp); the screen locked and the
  monitors off after a while without input, by default (swayidle and swaylock); a shadow under each window (`[shadow]`); optional
  rounded corners (`corner-radius`, `rounded-corners`: GTK's by default); your own colors (`[colors]`).
- **Animations**, short and subtle: windows opening, closing, minimizing, maximizing and
  snapping; launchers and notifications; workspaces sliding. Configurable, or off.
- **Window and layer rules** by app id, title or namespace: animations, workspace, size,
  position, maximized or fullscreen, always on top, on all workspaces, title bar, corner radius,
  opacity, blur and shadow per app; blur and animations per panel or launcher.
- **Monitors:** several at once, hotplugged, each with its own mode, position, rotation,
  (fractional) scale and adaptive sync; a fullscreen app's frames sent to the screen as it drew
  them (direct scanout, `direct-scanout`); each frame drawn as close to the refresh as it safely
  can be, for less latency (`render-late`).
- **Apps get their windows back:** place, size, maximized or fullscreen, for apps that ask
  (xdg-session-management), even after Pila restarts.
- **X11 apps** through Xwayland, with the clipboard shared both ways. Xwayland starts when an
  X11 app needs it and, with `[xwayland] idle-timeout`, stops when none is left.
- **Desktop tools:** layer shell (waybar, fuzzel, mako, swaybg), session lock (swaylock),
  idle notification and inhibition (swayidle, video players), clipboard managers
  (ext-data-control), screenshots (grim; ext-image-copy-capture and wlr-screencopy), taskbars
  that act on windows
  (foreign-toplevel-management), night light (gamma control), screen off and monitor
  settings (output power and management), blur behind panels and windows (background-effect),
  workspace indicators and switchers (ext-workspace), a visual system bell, dialogs that stay
  above their window and modal ones that keep the focus (xdg-dialog, and X11's), window rules by tag
  (xdg-toplevel-tag), games with vsync off shown at once, torn (wp-tearing-control), surfaces an
  app fades without drawing them again (wp-alpha-modifier),
  the pointer moved by the app it's on (wp-pointer-warp: games, SDL 3), X11 apps that grab the
  keyboard (xwayland-keyboard-grab: virtual machines, remote desktops),
  Vulkan's FIFO presentation (wp-fifo, wp-commit-timing), explicit sync
  (linux-drm-syncobj), virtual keyboards (`wtype`, on-screen keyboards). Enough for a full
  desktop shell. Sandboxed apps (Flatpak) don't see the protocols that read the
  screen or act on other windows and input (wp-security-context).
- **The desktop portal, served by Pila** (its own backend: no xdg-desktop-portal-wlr): screen
  sharing for browsers, video calls and OBS over PipeWire, of a monitor or a single window (kept
  updating while it's covered, minimized or on another workspace), picked in a panel Pila draws
  with live previews; apps' screenshots, of every monitor when the portal lets the app take one
  unasked, else of the monitor or window you pick; a color picked from the screen (GTK's color
  chooser, GIMP); and apps' global shortcuts (push-to-talk, a recorder's start and stop), whose
  keys you confirm or change in a panel and Pila remembers for each app. Nothing is shared or
  taken without your say, a shared window shows nothing new while the screen is locked, and
  shortcuts do nothing then.
- **Input:** keyboard layout, repeat and Num Lock at startup, touchpad and mouse settings (tap, natural scrolling,
  speed, disable while typing, ...), three-finger swipes to switch windows and maximize,
  four-finger swipes to switch workspaces (configurable), other touchpad gestures for apps,
  pointer lock and confinement for games, shaking the pointer to find it (`[cursor]`), any key
  bindable (media keys included, repeating while held if asked), and the mouse wheel with
  modifiers; input methods such as fcitx5 for Chinese, Japanese and Korean (input-method-v2,
  text-input-v3).
- **Configuration** in one TOML file, applied live when saved.
- **Scripting:** `pila msg` lists windows, monitors and workspaces as text or JSON and runs any
  action ([IPC.md](IPC.md)).

## Where it fits

- **Stacking, with server-side decorations:** overlapping windows, xdg-decoration with Pila's
  own title bars and resize borders (client-side ones honored), snapping, workspaces spanning
  all outputs. Shells built on layer shell usually run on tiling compositors (Hyprland,
  sway); Pila gives them a floating desktop.
- **A compositor, not a desktop environment:** bar, launcher, notifications, wallpaper and lock
  screen are external clients (layer shell, ext-session-lock, ...): standard tools such as
  waybar and fuzzel by default, any other one in their place. Pila draws only what a
  compositor must or does best itself: title bars and the window switcher, with live previews
  (its IPC tells another one what to show). The floating modes of KDE
  Plasma, GNOME and COSMIC come with their own shell; labwc keeps Openbox's model and XML
  config; Wayfire is built around Compiz-style plugins. Pila has one TOML file and an IPC
  socket.

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
  by an opaque one isn't processed; one on another workspace or minimized keeps its backdrop,
  so showing it again allocates nothing.
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
- **Rendering late:** on the DRM backend, while the scene is steady, a frame is drawn as close to
  its vblank as the output's recent frames allow (the worst of the last 128, CPU and GPU, plus
  the other outputs' and a slack, never under half a period), so commits and pointer motion
  that arrive meanwhile make that vblank. Not during Pila's animations or for 300 ms after a
  window or layer surface appears or a window changes size (apps busy on the GPU then), nor
  with tearing or adaptive sync; a held frame is let go at once if the scene stops being
  steady, and one that still comes late turns it off on that output for two seconds. On the
  development laptop: 25.8 to 21.3 ms and 28.5 to 21.7 ms from a redraw asked for to the vblank
  that showed it, no refresh missed with two monitors or the laptop's screen alone on battery.
  `render-late = false` turns it off.
- **Measured, not assumed:** `RUST_LOG=pila::frames=debug` logs, per output every 5 s, frame
  count, mean and worst CPU time, the GPU's time (from submission to the native fence's signal,
  stamped by the kernel and read without waiting), missed refreshes, idle redraws and
  direct-scanout frames. On bare metal, missed refreshes are counted from the DRM vblank
  sequence, and each late frame is attributed: held back by the idle wait, drawn late, or shown
  late after being submitted in time; the frames' latency (from a redraw asked for to the
  vblank that showed it) and, for frames rendered late, their margin and how long before the
  vblank they were ready are logged too. Captures and the event loop's own housekeeping are
  logged apart.
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
exists), implemented in Pila when Smithay lacks them. wlroots protocols only where shells or
input methods need them and no `ext-` replacement exists yet (layer-shell, screencopy,
foreign-toplevel-management, gamma-control, output-management, output-power-management,
virtual-keyboard, input-method-v2), each to be dropped when one lands. No KDE, GNOME or Hyprland protocols.

Among the less common ones: xdg-session-management (window placement and state restored across
client and compositor restarts), ext-background-effect, ext-workspace, ext-image-copy-capture,
ext-data-control, xdg-dialog, xdg-toplevel-tag, xdg-system-bell, wp-fifo, wp-commit-timing,
wp-alpha-modifier, wp-pointer-warp, wp-tearing-control, xwayland-keyboard-grab, linux-drm-syncobj, wp-security-context, cursor-shape, pointer-constraints and
relative-pointer, idle-notify and idle-inhibit, text-input-v3. Xwayland runs through Smithay's X11 window
manager, restarted if it dies, with selections bridged both ways.

## Security model

- **Privileged globals filtered by security context:** clients connected through
  wp-security-context (Flatpak) can't see screen capture (both protocols), data-control,
  layer shell, session lock, foreign-toplevel list and management, gamma, output management and
  power, ext-workspace, virtual keyboard, input method, keyboard-shortcuts-inhibit, idle-notify or
  security-context itself. Any new global that exposes or controls other clients, input or
  outputs is filtered too, by policy.
- **Session lock enforced at the focus types:** while locked, the keyboard and pointer focus
  types themselves refuse any target but a lock surface, so input can't reach another client
  through any path, Pila's or a Smithay grab's; a path that tries is a panic in debug builds.
  Hit testing and the scene contain only lock surfaces, and a test runs every action, gesture,
  activation and taskbar request while locked.
- **xdg-activation tokens** only for a serial newer than the client's keyboard focus, or a
  click on it since the focus last moved (a clicked notification; hovering alone doesn't count),
  single-use and short-lived; one from before a lock activates nothing.

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

## Not there yet

Pila is alpha. Missing: touchscreens and tablets (tablet-v2). NVIDIA has been tried on one card
(a GTX 1060 with the proprietary driver); secondary-GPU outputs are off by default and untested on
real hardware. See [Status](../README.md#status), the [roadmap](ROADMAP.md) and, if you can, [help test
it](../README.md#help-test-pila).
