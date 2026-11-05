# Changelog

## Unreleased

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
  notification, not only in the log.
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
