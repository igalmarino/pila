# Roadmap

This is Pila's roadmap: what's planned next, what is waiting on hardware testers or upstream
projects, and what is explicitly out of scope. It is updated with each release; see
[CHANGELOG.md](https://github.com/igalmarino/pila/blob/main/CHANGELOG.md) for completed work.

Last updated for **0.9.4** (2026-11-23). Pila is currently alpha and usable day to day on its
development machine (covering window management, Xwayland, workspaces, snapping, rules, input
methods, screen capture, desktop shells and tools like waybar, and bare-metal rendering
with direct scanout), backed by 1,231 tests and real-app checks. What remains is mostly hardware
testing and protocols Pila doesn't implement yet.

## Now: the next release

Small items that can be tested in CI without new hardware.

Nothing currently queued; 0.9.4 shipped the previous batch.

## Next

User-requested features get priority here.

- **More built-in desktop portals**
  ([discussion #12](https://github.com/igalmarino/pila/discussions/12)): Screen sharing has been
  built into Pila since 0.9.0, followed by single-window sharing, app screenshots, the color
  picker, and global shortcuts in 0.9.1. Next up, if apps need it: the `RemoteDesktop` portal.

## Waiting for hardware and testers

Pila is developed on a single AMD laptop with an external monitor. These items are either
implemented or ready to work on once someone can test them on real hardware; comment on
[issue #1](https://github.com/igalmarino/pila/issues/1) if you can help.

- **Monitors on a second GPU** (`other-gpus = true`, disabled by default): Untested on a real
  dual-GPU machine; direct scanout on secondary-GPU outputs will follow once basic multi-GPU is
  verified.
- **NVIDIA** beyond the one tested GTX 1060, as well as hybrid-graphics laptops.
- **Touchscreens** (`wl_touch`): Currently, touch events only count as idle-resetting activity.
- **Drawing tablets** (`tablet-v2`).
- **VR headsets** (`wp-drm-lease-v1`): Only if there is user demand.

## Waiting for other projects

- **`xdg-session-management` in `wayland-protocols`:** Once a crate release includes the generated
  module, Pila will drop its local scanner bindings (`src/session_protocol.rs`).
- **Standard replacements for wlroots protocols:** Each accepted wlroots protocol (layer shell,
  screencopy, foreign toplevel management, gamma control, output management and power, virtual
  keyboard, and input method) will be replaced by its `ext-` successor once available in
  `wayland-protocols` and Smithay.
- **Upstreaming Smithay fork patches:** Once Smithay merges the fixes Pila currently carries
  (zero-width text cursors, async page flips, reserving X displays separately from Xwayland,
  striped shm uploads, and looking up `xdg-foreign` surfaces), Pila will switch back to upstream
  Smithay.
- **Direct scanout of OpenGL apps on AMD GPUs:** On AMD Renoir with Mesa 26.3.1 (`radeonsi`),
  fullscreen OpenGL apps (such as `kitty` or `weston-simple-egl`) keep a render-only compressed
  (DCC) buffer layout that the display engine cannot scan out, even after Pila advertises
  scanout-compatible formats via dmabuf feedback, forcing composited fallback. Pila logs these
  rejections under `pila::frames` (`direct scanout refused`, `dmabuf feedback`).
- **Post-close frame stall after torn fullscreen games on AMD APUs:** When a fullscreen game with
  vsync off (`vkcube`) exits on bare-metal `amdgpu`, Pila's next frame stalls for ~2.4 ms on the
  GPU copy engine (`sdma0`) while the kernel migrates or clears the game's memory and page tables
  wait (measured across 8 traces; the frame itself takes ~1 ms). At peak GPU clocks the stall
  halves, and with vsync enabled it doesn't happen. Roughly once in ten closes, it lands right
  before vblank and drops a refresh. This happens inside the driver, outside Pila's control.
- **Maintained font parser in `cosmic-text`:** `cargo audit` flags `ttf-parser` (pulled in by
  `cosmic-text`) as unmaintained (`RUSTSEC-2026-0192`, no security vulnerability). Pila will pick
  up the replacement in a future `cosmic-text` release.

## Not planned

- **Built-in desktop shell components:** Pila will not ship its own bar, launcher, on-screen
  display, wallpaper daemon, idle daemon, or screenshot tool. Standard tools (`waybar`, `fuzzel`,
  `swayosd`, `swaybg`, `swayidle`, `grim`, `slurp`) handle those well. Pila only draws what belongs
  in the compositor: title bars, the window switcher, and the overview.
- **Vendor protocols:** KDE, GNOME, and Hyprland protocols won't be added even if Smithay supports
  them (wlroots protocols are accepted only under the policy in
  [CONTRIBUTING.md](https://github.com/igalmarino/pila/blob/main/CONTRIBUTING.md)). Features
  without a standard protocol go through Pila's config and `pila msg`. GNOME's private D-Bus
  interfaces (Mutter's `ScreenCast` and `DisplayConfig`, GNOME Shell's `Screenshot`) are also out
  of scope.
- **Builds without X11 support:** Setting `[xwayland] enabled = false` and leaving Xwayland
  uninstalled already gives a completely X11-free session, and Pila's X11 code is pure Rust with no
  X library dependencies.
- **Breaking config or IPC changes:** Config keys, actions, and `pila msg` fields are
  additive-only (see *Compatibility* in `CONTRIBUTING.md`).
