# Changelog

## 0.2.2 (2026-11-06)

- **Packages:** each release now has a `.deb` (Ubuntu 24.04 or newer, Debian 13 or newer) and
  a `.rpm` (Fedora). They're installed on clean systems and started headless before they're
  attached, but a real session on these distros hasn't been tried yet.
- **Nix:** the repository is a flake: `nix run github:igalmarino/pila`, confirmed on a NixOS
  system by a tester. It can also be a login session on NixOS (not tried yet), and `nix develop`
  gives a build environment.
- **`pila-report`** gathers what a bug or test report needs into a file you can read first:
  version, system, GPU, monitors, whether the config is valid, and the end of the log, without
  your home directory, user name and host name. It sends nothing anywhere.
- **Fixes:** a missing `libEGL.so.1` is now a clear error that says what to do, not a crash.
- **Tests:** a test that failed now and then under load, seen once when building `pila-git`, now
  clicks where a snapped window can't be.

## 0.2.1 (2026-11-05)

- **New protocols:** xdg-session-management (apps restore their windows' place, size and
  maximized or fullscreen state, also after pila restarts), xdg-dialog (modal dialogs
  stay over their window), xdg-toplevel-tag (window rules can match a tag), wp-fifo and
  wp-commit-timing (frame pacing for games and video), linux-drm-syncobj (explicit sync,
  as Vulkan apps use; untested on NVIDIA), and a dmabuf client's rendering is waited for before
  it's shown.
- **Windows:** dialogs and other transient windows stay above their parent and follow
  it to other workspaces.
- **Monitors:** make, model and serial come from the monitor's EDID, for clients and for
  `[[output]]` sections that name a monitor (`name = "LEN P27h-30"`) instead of a
  connector. A monitor plugged in while on another VT comes on when you're back.
- **Security:** sandboxed apps (Flatpak) no longer see ext-workspace,
  keyboard-shortcuts-inhibit or ext-idle-notify: they can't switch workspaces, take
  pila's shortcuts away or watch when you're active. What an app can leave in the saved
  sessions is bounded. The session log directory is private.
- **Reliability:** Xwayland starts again if it dies (at most three times a minute), and
  starts with no signals blocked; a display tool's output configuration that the monitor
  can't take fails instead of being reported as applied; a client binding a monitor as
  it's unplugged is no longer killed; a pila run next to the login session's
  (`scripts/tty`) no longer overwrites its saved sessions.
- **Blur:** right at fractional scales (no sharp lines between strips), after a change
  of scale, in screenshots of a region and behind two overlapping surfaces; damage far
  from a blurred region no longer redoes it, and a monitor turned off gives its memory
  back.
- **Fixes:** a fullscreen window only covers the panels of its own monitor; a menu over
  a title bar gets the pointer; closing a window during Mod+Tab no longer skips one;
  focus no longer lands on an X11 menu.
- **Smoothness:** the cursor theme is read at startup on a thread, and screen captures
  wait for the GPU without holding up the event loop.
- **Tools:** `scripts/layer-namespaces` lists the layer surfaces your shell opens, to
  write `[[layer-rule]]` sections for them.

## 0.2.0 (2026-11-05)

- **Animations:** short and subtle, only where they help: windows rise and fade in and
  drop out when they open and close, shrink away when minimized, move and stretch when
  maximized, made fullscreen or snapped; launchers and notifications fade; workspaces
  slide sideways, each monitor on its own. `[animations]` turns them off, retimes them
  (`speed`) or sets each one's duration.
- **Window and layer rules:** `[[window-rule]]` matches windows by app id (WM_CLASS for
  X11 apps) and title, `[[layer-rule]]` layer surfaces by namespace, with `*` wildcards.
  Windows: animations, open on a workspace, open maximized or fullscreen, size, position,
  title bar, corner radius, opacity. Layer surfaces: animations (for shells that animate
  their own, e.g. `namespace = "noctalia-*"`) and blur.
- **Smoothness:** pila never makes a monitor miss a refresh while something moves, at the
  monitor's own rate; animations no longer stutter on bare metal, and the first one of a
  session no longer costs a long frame. `RUST_LOG=pila::frames=debug` logs frame times
  and missed refreshes per monitor.
- **`pila --check-config`** checks the config file, keyboard layout included, without
  starting pila (exit status 0 if it's valid, 1 if not).
- **Rounded corners:** optional, `corner-radius`.
- **Sandboxed apps** (Flatpak, through security-context) no longer see privileged
  protocols: layer shell, session lock, clipboard managers, screen capture, virtual
  keyboard, window and output management, gamma.
- **Fixes:** programs started by pila ignored `kill` and Ctrl+C (they inherited pila's
  blocked signals); a keyboard layout xkbcommon doesn't know is now shown in a
  notification, not only in the log; a covered or off-screen animating app no longer
  keeps the CPU busy; a window unmapped by its app (a null buffer) disappears and maps
  again like a new one; a window's screen capture stops when it closes; a window that
  asks to open maximized or fullscreen gets that size at once; a launcher or panel
  holding the keyboard shows over a fullscreen window; a minimized window is no longer
  shown as active; pointer locks and confinement end when keyboard focus goes, and a
  confined pointer can reach its app's menus; a symlinked config file reloads when its
  target changes; a stuck D-Bus or a gamma client can no longer freeze pila; pila
  starts on a system with no fonts (title bars without text) and creates the X11
  socket directory when it's missing; a monitor mode without a refresh rate no longer
  crashes the DRM backend; the session log is capped at 50 MiB.
- **Tests:** a headless backend (`--backend headless`) and an integration test suite
  that runs a real pila per test, in CI too; `tests/FEATURES.md` maps every feature to
  its tests. `scripts/check` runs real programs on bare metal and checks frame times.
- **Workspaces:** a configurable number (`workspaces`, default 4) spanning all monitors.
  Mod+1..N (one per workspace, up to 9) / Mod+Page_Up/Page_Down switch, with Shift they move the focused window;
  four-finger swipes left/right switch. Mod+Tab stays on the current workspace, and
  activating a window elsewhere (taskbar, xdg-activation) goes to its workspace. Panels
  see and switch them through ext-workspace.
- **Built-in screen off with an external monitor:** `disable-internal-display = true`
  turns a laptop's built-in screen (eDP, LVDS, DSI) off while an external monitor is on;
  it comes back when the external monitor is unplugged.

## 0.1.0 (2026-11-04)

The first public release: a stacking Wayland compositor for testing by others.

- **Windows:** server-side title bars (minimize, maximize, close) and resize borders, or
  client-side decorations; Mod+drag to move and resize; maximize and fullscreen; snapping
  to screen halves by keyboard or by dragging to an edge; keyboard move, resize and
  center; minimize; switching by recent use (Mod+Tab).
- **Monitors:** several at once, hotplug, per-monitor mode, position, rotation, adaptive
  sync and fractional scale; runtime changes through wlr-output-management; screens off
  through wlr-output-power-management.
- **X11 apps** through Xwayland, with clipboard and primary selection shared both ways.
- **Desktop protocols:** layer shell, session lock, idle notify and inhibit,
  ext-data-control, ext-image-copy-capture and wlr-screencopy (screenshots, screen sharing
  through xdg-desktop-portal-wlr), ext-foreign-toplevel-list and
  wlr-foreign-toplevel-management, wlr-gamma-control (night light), ext-background-effect
  (blur behind panels), xdg-system-bell, xdg-activation, xdg-foreign, pointer constraints,
  relative pointer, pointer gestures, keyboard-shortcuts-inhibit, virtual keyboard,
  viewporter, fractional scale, presentation time.
- **Input:** keyboard layout and repeat, touchpad and mouse settings, three-finger swipes
  for window switching and maximize, any key bindable.
- **Configuration** in one TOML file, applied live when saved. Mod defaults to Super on a
  TTY and Alt nested; the terminal defaults to the first installed of kitty, foot,
  alacritty and others.
- **Session:** a display-manager session entry and wrapper (`scripts/install`), a D-Bus
  environment for portals and PipeWire, with or without systemd; an Arch package
  (`packaging/aur/pila-git`).
