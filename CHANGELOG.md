# Changelog

## 0.1.0 (unreleased)

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
