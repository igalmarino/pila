# What sets Pila apart

Pila is a stacking Wayland compositor written in Rust on Smithay's main branch, designed to host
layer-shell desktops, whether assembled from standard tools (waybar, fuzzel, and mako by default) or
a full desktop shell. This page covers its features, design choices, and how it differs from other
compositors. For usage instructions, see the [user manual](MANUAL.md); for internal flows and
invariants, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Features

### Windows and workspaces
- **Decorations and resizing:** Server-side title bars (minimize, maximize, close) and resize
  borders, while respecting client-side decorations for apps that draw their own. Right-clicking a
  title bar opens a window menu.
- **Mouse and keyboard management:** `Mod` + drag to move or resize from anywhere in a window;
  maximize, fullscreen, minimize, and show desktop; center, move, and resize from the keyboard;
  keep a window always on top or visible on all workspaces; move windows and focus across monitors.
- **Snapping:** Snap windows to screen halves and quarters via keyboard shortcuts or by dragging to
  an edge or corner, with optional gaps (`gaps`).
- **Window switcher and overview:** Recent-use window switching (`Mod+Tab`, `Alt+Tab`) with live
  previews while held; an overview (`Mod+W`, configurable hot corner with optional pressure and
  delay, or four-finger swipe up) that spreads all workspace windows out live without overlap, plus
  a live workspace strip.
- **Workspaces:** Shared workspaces spanning all monitors (`Mod+1..N`, `Mod` + scroll wheel, or
  four-finger swipes).
- **Launcher and shortcuts:** Tap `Mod` alone to open the launcher (fuzzel by default). Built-in
  default bindings for volume, brightness, media, and screenshots (`wpctl`, `brightnessctl`,
  `playerctl`, `grim`, `slurp`), plus automatic idle locking and monitor power-off via `swayidle`
  and `swaylock`. Press `Mod+?` (`show-shortcuts`) to view your current shortcuts at any time; a
  short welcome cheatsheet appears on first run.
- **Visual styling:** Window shadows (`[shadow]`), rounded corners (`corner-radius`,
  `rounded-corners`, top-only like GTK by default), custom color schemes (`[colors]`), amber title
  bars for urgent windows, and subtle configurable animations for windows, layers, and workspace
  slides.

### Rules, monitors, and sessions
- **Window and layer rules:** Match by app ID, title, tag, or layer namespace to control
  animations, initial workspace or output, size, position, maximized/fullscreen state,
  always-on-top, all-workspaces, title bar, corner radius, opacity, blur, and shadow.
- **Multi-monitor support:** Hotplugging, per-monitor modes, positions, rotations, fractional
  scaling, and adaptive sync (VRR). Supports direct scanout (`direct-scanout`) for fullscreen apps
  and late rendering (`render-late`) to cut display latency.
- **Session restore:** Restores window position, size, and maximized/fullscreen state for apps that
  support `xdg-session-management`, even across compositor restarts.
- **X11 compatibility:** On-demand Xwayland with bidirectional clipboard sharing. Optionally shuts
  down when idle (`[xwayland] idle-timeout`).

### Desktop tools, portals, and input
- **Desktop protocols:** Layer shell (`waybar`, `fuzzel`, `mako`, `swaybg`), session lock
  (`swaylock`), idle notification and inhibition (`swayidle`, video players), clipboard managers
  (`ext-data-control`), screenshots (`ext-image-copy-capture`, `wlr-screencopy`), taskbars
  (`foreign-toplevel-management`), night light (gamma control), output power and management,
  background blur (`ext-background-effect`), workspace indicators (`ext-workspace`), visual system
  bell, transient/modal dialogs (`xdg-dialog` and X11), window tags (`xdg-toplevel-tag`), tearing
  control (`wp-tearing-control`), surface alpha (`wp-alpha-modifier`), pointer warping
  (`wp-pointer-warp`), Xwayland keyboard grabs (`xwayland-keyboard-grab`), Vulkan FIFO and commit
  timing (`wp-fifo`, `wp-commit-timing`), explicit sync (`linux-drm-syncobj`), and virtual keyboards
  (`wtype`). Sandboxed apps (Flatpak) are hidden from privileged protocols via
  `wp-security-context`.
- **Built-in desktop portal:** Pila implements its own `xdg-desktop-portal` backend (no
  `xdg-desktop-portal-wlr` required):
  - *Screen sharing* over PipeWire for browsers, video calls, and OBS: pick a monitor, a custom
    screen region (outlined in the accent color while shared), or a single window (kept live even
    when covered, minimized, or on another workspace) from a picker with live previews, with an
    optional **Remember** toggle to reuse the same share without re-prompting.
  - *App screenshots* (either all monitors when permitted without prompting, or chosen via the same
    picker).
  - *Color picker* for GTK, GIMP, Inkscape, and similar apps.
  - *Global shortcuts* (such as push-to-talk or recording hotkeys), confirmed and persisted per app.
  Nothing is shared or captured without your approval, shared windows pause updates while locked,
  and global shortcuts are disabled on the lock screen.
- **Input:** Keyboard layouts, repeat rate, and startup Num Lock; libinput touchpad and mouse
  settings; configurable 3-, 4-, and 5-finger touchpad swipes (other gestures pass through to
  apps); pointer lock and confinement for games; shake-to-find cursor (`[cursor]`); bindable media
  keys (with optional key repeat) and `Mod` + scroll wheel; CJK input methods such as `fcitx5`
  (`input-method-v2`, `text-input-v3`).
- **Configuration and IPC:** Single TOML config file with live reload on save, and `pila msg` for
  querying state (text or JSON), subscribing to events, and running actions ([IPC.md](IPC.md)).

## Where it fits

- **Stacking with server-side decorations:** Overlapping windows, `xdg-decoration` with built-in
  title bars and resize borders (while honoring client-side decorations), snapping, and workspaces
  that span all monitors. Layer-shell desktop setups usually target tiling compositors like
  Hyprland or sway; Pila brings the same ecosystem to a floating desktop.
- **A compositor, not a desktop environment:** The bar, launcher, notifications, wallpaper, and
  lock screen are separate Wayland clients (waybar and fuzzel by default, easily swapped for
  anything else). Pila only draws what belongs in the compositor: title bars, the window switcher,
  and the overview with live previews (and its IPC exposes switcher state for external switchers).
  Compared to alternatives: KDE Plasma, GNOME, and COSMIC bundle their own desktop shells; labwc
  follows Openbox's model and XML configuration; Wayfire centers on Compiz-style plugins. Pila
  stays small: one TOML file and a Unix IPC socket.

## Performance model

Pila's main performance goal is simple: **never drop a frame when there is something new to show**
(an animation, a window drag, or a client commit) at each monitor's refresh rate, keeping CPU work
under 4 ms per frame per output (release build on an integrated AMD GPU). When nothing changes,
Pila doesn't redraw.

- **Per-output, on-demand redraws:** Changes only request redraws on the outputs they actually
  affect (a window's bounding box, the outputs a committed surface touches, or the pointer's old
  and new output). Nothing redraws on a fixed timer, and each output's render list skips windows
  that don't intersect it. In the test suite, if an output's visual content changes without a
  matching redraw request, Pila panics, preventing missed redraws from slipping in.
- **Damage tracking everywhere:** Per output, per capture session, and per blurred region, plus
  buffer-age partial repaints when running nested.
- **Idle-redraw throttling:** If a redraw finds nothing new (for instance, a client committing
  unchanged buffers in a frame-callback or FIFO loop, or pointer movement with no visible cursor),
  the next redraw on that output waits until half a refresh period before the next vblank. Clients
  cannot force Pila to spin faster than the refresh rate, while compositor-initiated changes never
  wait.
- **Hidden surfaces throttled:** Frame callbacks go only to surfaces visible on the output being
  drawn. Completely hidden surfaces get one callback per second so covered apps don't spin.
- **Change-driven blur:** Dual-Kawase blur (`ext-background-effect` or per-window rules) renders
  only the windows and layers intersecting the blurred region, tracks damage independently, and
  re-runs blur passes only when the backdrop changes. Covered windows are skipped, and windows on
  other workspaces or minimized keep their cached backdrop so switching back doesn't reallocate.
- **Non-blocking event loop:** Font loading and shaping, cursor theme loading, D-Bus environment
  exports, shared-memory capture writes, and GPU capture waits all run on background threads or
  behind fences. CPU-drawn client buffers upload on commit in 64 KiB strips rather than mid-frame,
  and one-time costs (shader compilation, texture upload warm-up, workspace-slide offscreen
  textures, common glyphs) are paid at startup. Title bar textures are cached across resizes and
  focus changes, and event-loop housekeeping skips subsystems with no active work.
- **Direct scanout:** On the DRM backend, when a single unobscured client buffer covers a monitor
  (such as a fullscreen game or video), Pila puts it straight on the primary plane without
  compositing. Eligible surfaces receive per-surface dmabuf feedback advertising the plane's
  scanout formats so clients can allocate compatible buffers. Enabled by default on primary-GPU
  outputs (`direct-scanout = false` disables it).
- **Late rendering:** On the DRM backend, when the scene is steady, Pila holds each frame and draws
  it as close to the vblank as recent frame timings safely allow (using the worst CPU + GPU time
  from the last 128 frames, plus other outputs and safety slack, capped at half a refresh period).
  This lets commits and pointer motion that arrive mid-frame hit the upcoming refresh instead of
  waiting for the next one. Late rendering pauses during animations, for 300 ms after windows or
  layers open or resize, and during tearing or adaptive sync. If the scene becomes busy, a held
  frame is submitted immediately; if a frame still arrives late, late rendering backs off on that
  output for two seconds. On the development laptop, this cut redraw-to-vblank latency from 25.8 ms
  to 21.3 ms on the internal panel and from 28.5 ms to 21.7 ms on the external monitor with zero
  dropped refreshes (both plugged in with two monitors and on battery with one). Disable with
  `render-late = false`.
- **Measured continuously:** `RUST_LOG=pila::frames=debug` logs per-output stats every 5 seconds:
  frame count, mean and worst CPU time, kernel-timestamped GPU time (via native fences without
  blocking), missed refreshes, idle redraws, and direct-scanout counts. On bare metal, missed
  refreshes are measured directly from DRM vblank sequences, with late frames categorized by cause
  (idle wait, late draw, or late presentation) alongside latency and late-render margins. Captures
  and event-loop housekeeping are logged separately. `scripts/check` runs real apps on a TTY and
  validates these metrics before rendering, input, or event-loop changes are committed.

## Client pacing and buffers

- **Slow clients only delay themselves:** A commit carrying a dmabuf that the client GPU is still
  rendering is held by a pre-commit blocker (via its implicit fence or `linux-drm-syncobj` acquire
  point) and applied once signaled. Pila never stalls a frame waiting on a slow client.
- **wp-fifo:** Barriers signal on the frame that displays the surface, pacing FIFO clients to one
  update per refresh. Barriers on hidden surfaces or powered-off outputs release once per second.
- **wp-commit-timing:** Timestamped commits are applied for the frame predicted to scan out at that
  time, calculated from kernel vblank timestamps.
- **Presentation feedback:** Uses kernel timestamps on `CLOCK_MONOTONIC` throughout.
- **Bounded geometry:** Every client-provided size and offset (buffers, subsurface positions,
  viewport destinations, window geometry, layer-shell dimensions, margins, exclusive zones, and
  popup positioners) is validated against a 32,768 logical-pixel limit before arithmetic to prevent
  overflows in Pila or Smithay; out-of-range values raise protocol errors.

## Screen capture

- **Protocols:** Implements `ext-image-copy-capture` (output and toplevel sources) on the official
  `wayland-protocols` bindings, plus `wlr-screencopy` for older clients.
- **No double compositing on bare metal:** On primary-GPU outputs, an output capture blits the frame
  the DRM compositor just rendered, reusing the session's damage over the same elements.
- **Zero-copy and asynchronous:** Frames render directly into the client's dmabuf only when the
  capture session detects damage. Completion signals via a native fence fd on the event loop
  without blocking on the GPU. Shared-memory clients fall back to read-back.
- **A/V sync:** Frame timestamps report when the content actually scans out (the next vblank), not
  when the copy was triggered.
- **Live hidden windows:** A captured window that is covered, minimized, or on another workspace
  receives frame callbacks at the capture client's rate so window shares don't stall at 1 fps.

## DRM/KMS backend

- **Per-output failure isolation:** If a DRM render or commit fails (or DRM master is lost during a
  VT switch), only that output backs off and retries exponentially; other monitors keep running.
- **Mode fallback:** If a monitor's preferred mode exceeds link bandwidth, Pila tests lower refresh
  rates at the same resolution and then smaller resolutions using atomic test commits.
- **DisplayPort link retraining:** Automatically retrains outputs when the kernel marks
  `link-status` as bad (common after resume or through docks, including across VT switches),
  falling back to a lighter listed mode if the link can no longer carry the previous one.
- **Safe hotplug:** Removing an output disables its `wl_output` global first and destroys it later,
  preventing protocol errors from clients binding it mid-removal.
- **Scanout-confirmed screen lock:** `ext-session-lock` is confirmed only after every enabled
  output has scanned out a locked frame at vblank, guaranteeing no unlocked frame can appear after
  the locker is notified.
- **Multi-GPU (experimental):** With `other-gpus = true`, outputs on secondary GPUs are rendered on
  the primary GPU via Smithay's `MultiRenderer` and copied across via dmabuf import or system
  memory. Software renderers (`llvmpipe` on KMS) are detected and kept off the dmabuf path. See
  [MULTI_GPU.md](MULTI_GPU.md).
- **Stall-free gamma:** `wlr-gamma-control` ramps submit as non-blocking atomic commits of
  `GAMMA_LUT` alone (taking one vblank like a regular frame), avoiding the blocking legacy ioctl
  that makes night-light fades stutter on atomic drivers.
- **VRR and output power:** Per-output adaptive sync, and output power (`wlr-output-power-management`)
  implemented by clearing and restoring the CRTC.

## Protocols

Pila prioritizes standard `wayland-protocols` (stable, staging, then unstable when no newer
equivalent exists), implementing them directly when Smithay doesn't yet provide them. wlroots
protocols are included only when desktop shells or input methods require them and no `ext-`
replacement exists (`layer-shell`, `screencopy`, `foreign-toplevel-management`, `gamma-control`,
`output-management`, `output-power-management`, `virtual-keyboard`, and `input-method-v2`), and
will be replaced as standard equivalents land. Vendor protocols (KDE, GNOME, Hyprland) are not
added.

Beyond the basics, supported protocols include `xdg-session-management`, `ext-background-effect`,
`ext-workspace`, `ext-image-copy-capture`, `ext-data-control`, `xdg-dialog`, `xdg-toplevel-tag`,
`xdg-system-bell`, `wp-fifo`, `wp-commit-timing`, `wp-alpha-modifier`, `wp-pointer-warp`,
`wp-tearing-control`, `xwayland-keyboard-grab`, `linux-drm-syncobj`, `wp-security-context`,
`cursor-shape`, `pointer-constraints`, `relative-pointer`, `idle-notify`, `idle-inhibit`, and
`text-input-v3`. Xwayland runs through Smithay's X11 window manager, restarts automatically if it
crashes, and bridges clipboards in both directions.

## Security model

- **Security-context filtering:** Clients connected via `wp-security-context` (such as Flatpak
  apps) cannot see privileged globals: screen capture (both protocols), data-control, layer shell,
  session lock, foreign-toplevel list and management, gamma, output management and power,
  `ext-workspace`, virtual keyboard, input method, keyboard-shortcuts-inhibit, idle-notify, or
  security-context itself. Any new global that observes or controls other clients, input, or
  outputs must be filtered the same way.
- **Type-level session lock enforcement:** While locked, the keyboard and pointer focus types
  themselves reject any target other than a lock surface, blocking input leaks across both Pila and
  Smithay grabs (and panicking in debug builds if any code path tries). Hit testing and the render
  scene include only lock surfaces, and an integration test runs every action, gesture, activation,
  and taskbar request while locked.
- **Strict activation tokens:** `xdg-activation` tokens require a serial newer than the client's
  keyboard focus, or a click on the client since focus last changed (hovering alone does not
  qualify). Tokens are single-use, short-lived, and invalidated by locking the screen.

## Configuration and IPC

- **Live-reloaded TOML:** Watched via `inotify` on the config directory (including symlinked
  dotfiles). Invalid edits keep the running config active and trigger a notification pointing to
  the line and error.
- **Offline validation:** `pila --check-config` validates the config file and compiles the XKB
  keymap, exiting `0` or `1` (useful in dotfiles CI or pre-commit hooks).
- **Dynamic rules:** Window rules match on app ID, title, or `xdg-toplevel-tag`, and layer rules
  match on namespace. Rules re-evaluate on title changes and config reloads.
- **Unix socket IPC:** Served on the event loop. `pila msg` queries windows, outputs, workspaces,
  colors, keyboard layouts, and remembered screen shares in plain text or JSON, runs any bindable
  action, and streams live changes via `subscribe`. See [IPC.md](IPC.md).

## Testing and reliability

- **Headless integration suite:** Each test launches a real Pila instance on virtual outputs (GPU
  or `llvmpipe` in CI), connects Wayland clients, and injects input over a control socket, polling
  conditions rather than relying on fixed sleeps.
- **Synced feature inventory:** [`tests/FEATURES.md`](../tests/FEATURES.md) maps every feature and
  invariant to its tests. A test verifies that every listed test exists and every integration test
  is listed. Additional tests check that docs only reference existing code symbols and that the
  manual and man page cover every default binding and CLI flag.
- **Regression tests first:** Bug fixes begin with a failing test that reproduces the issue.
- **Fuzzing (`cargo-fuzz`):** Targets for the config, IPC request, EDID, gamma-ramp, saved
  app-shortcut, and saved-session parsers, plus arbitrary protocol request sequences against a live
  Pila instance.
- **DRM backend in CI:** A workflow tests the udev backend on the Linux kernel's `vkms` virtual
  display driver, covering frames, gamma, output hotplug, and hotplugging a second `vkms` GPU.
- **No unexpected panics:** After startup, the code avoids unchecked `unwrap`/`expect` calls;
  poisoned mutexes and missing user data are handled gracefully, and any crash logs a backtrace.

## Portability

Pila makes no assumptions about distro, init system, or display manager. It supports logind,
elogind, and seatd, detects optional tools (`systemd-cat`, `logger`, `dbus-run-session`, per-user
D-Bus) at runtime, and uses configurable install paths. Packages exist for Arch (AUR), Debian,
Ubuntu, Fedora, and Nix, and CI builds across all of them as well as with the minimum supported
Rust version (`rust-version`).

## Not there yet

Pila is still alpha. Missing features include touchscreens and drawing tablets (`tablet-v2`).
NVIDIA has only been tested on one GTX 1060 with the proprietary driver, and secondary-GPU outputs
remain off by default until tested on real multi-GPU hardware. See
[Status](../README.md#status), the [roadmap](ROADMAP.md), and
[Help test Pila](../README.md#help-test-pila) if you can test on other hardware.
