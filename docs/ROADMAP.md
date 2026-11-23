# Roadmap

Pila's one roadmap: what comes next and in what order, what waits for testers or for other
projects, and what isn't planned. It changes with each release; the
[changelog](https://github.com/igalmarino/pila/blob/main/CHANGELOG.md) says what's done.

Last updated for **0.9.2** (2026-11-22). Pila is alpha and usable day to day on its development
machine: windows, Xwayland, workspaces, snapping, rules, input methods, screen capture, desktop
shells and tools (waybar and friends), bare metal with direct scanout, all covered by
1216 tests and real-app checks. What's left is mostly hardware Pila hasn't met and protocols it
doesn't speak yet.

## Now: the next release

Small, testable in CI, no new hardware needed.

Nothing open: 0.9.2 took what was here.

## Next

A feature users ask for goes here first.

- **The first frames after a game tears cost the GPU 3.5-4.8 ms** (bare metal, amdgpu): when a
  fullscreen game with vsync off (`vkcube`) closes or leaves fullscreen, Pila's next frames take
  that long on the GPU, against about 1 ms after the same game with vsync; asked for 4-5.5 ms
  before a vblank, one misses it (about one `scripts/check` in ten, on either monitor). Not the
  wallpaper, the animations, the game's process ending, nor waiting behind its work (a
  high-priority GPU context changed nothing): what's left is in the driver or the switch back
  from asynchronous flips, which needs a GPU trace (amdgpu tracepoints) to tell.
- **More of the desktop portal inside Pila** ([discussion #12](https://github.com/igalmarino/pila/discussions/12)):
  screen sharing is Pila's own since 0.9.0; windows, apps' screenshots, the color picker and apps'
  global shortcuts too since 0.9.1. Next, if apps ask for them: a region of the screen to share
  or take, and the RemoteDesktop portal.

## Waiting for hardware and testers

Pila is developed on one AMD laptop with an external monitor. These are written, or ready to be,
once someone can try them; [issue #1](https://github.com/igalmarino/pila/issues/1) is the place
to say so.

- **Monitors wired to a second GPU** (`other-gpus = true`, off by default): untried on a real
  two-GPU machine; then direct scanout on those monitors.
- **NVIDIA** beyond one GTX 1060, and hybrid laptops.
- **Touchscreens** (`wl_touch`): touches only count as activity today.
- **Drawing tablets** (`tablet-v2`).
- **VR headsets** (`wp-drm-lease-v1`): only if someone asks.

## Waiting for other projects

- **xdg-session-management** in a release of the `wayland-protocols` crate: Pila then drops the
  bindings it generates itself (`src/session_protocol.rs`).
- **Standard replacements for the wlroots protocols** Pila accepts (layer shell, screencopy,
  foreign toplevel management, gamma, output management and power, virtual keyboard, input
  method): each is swapped for its `ext-` version once one lands in wayland-protocols and
  Smithay.
- **Smithay with the changes Pila's fork carries** (a text cursor with no width, asynchronous
  page flips, the X display reserved apart from Xwayland, shm buffers uploaded in strips, the
  surface an xdg-foreign handle names): Pila goes back to Smithay's own repository.
- **Direct scanout of EGL apps on AMD GPUs**: a fullscreen app drawn with OpenGL (kitty,
  weston-simple-egl) keeps the render-only compressed (DCC) buffer layout Mesa 26.3's radeonsi
  picks, which the display can't read, after Pila tells it the layouts the screen takes (a
  scanout tranche of dmabuf feedback), so Pila draws its frames instead of showing them as they
  are. Seen on a Renoir laptop with Mesa 26.3.1; Pila's part is logged (`direct scanout
  refused`, `dmabuf feedback` on `pila::frames`).
- **A maintained font parser in cosmic-text**: `cargo audit` reports `ttf-parser`, which
  cosmic-text uses, unmaintained (RUSTSEC-2026-0192; no vulnerability). Moving up with
  cosmic-text's next release.

## Not planned

- A bar, launcher, on-screen display or screenshot tool of Pila's own: standard tools (waybar,
  fuzzel, swayosd, grim and slurp, ...) do those jobs, and Pila is the compositor alone. Pila
  draws what a compositor must or does best itself: title bars, the window switcher and the overview. Idle
  (swayidle) and the wallpaper (swaybg) are standard tools' too.
- Vendor protocols (KDE, GNOME, Hyprland, …), even when Smithay implements them; wlroots ones only
  as the protocol policy in
  [CONTRIBUTING.md](https://github.com/igalmarino/pila/blob/main/CONTRIBUTING.md) allows.
  Features with no protocol go through Pila's configuration and `pila msg`. Nor GNOME's private
  D-Bus interfaces (Mutter's ScreenCast and DisplayConfig, GNOME Shell's Screenshot), which
  xdg-desktop-portal-gnome needs to capture the screen.
- A build without X11 support: `[xwayland] enabled = false` and Xwayland left uninstalled give a
  session with no X at all, and the X11 code Pila compiles in is pure Rust, with no X libraries.
- Breaking configuration or `pila msg`: keys, actions and fields are only added to
  ("Compatibility" in CONTRIBUTING.md).
