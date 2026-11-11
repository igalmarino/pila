# What sets Pila apart

There are many Wayland compositors. This page says what Pila does differently, so you can tell
whether it's the one for you. What it does is in the [README](../README.md#features) and the
[user manual](MANUAL.md); what it doesn't do yet is at the end of this page.

## A classic desktop for modern shells

Shells such as [Noctalia](https://noctalia.dev) and Dux Shell are
usually run on tiling compositors (niri, Hyprland, sway). Pila gives them a **stacking**
desktop instead: windows float and overlap, with server-side title bars (minimize, maximize,
close), resize borders, shadows and snapping to halves and quarters, the way most desktops
work.

Pila is only the compositor: the bar, launcher, notifications, wallpaper and lock screen come
from the shell or from the usual Wayland tools (waybar, fuzzel, mako, swaybg, swaylock). Unlike
the floating modes of full desktops (KDE Plasma, GNOME, COSMIC), it brings no desktop of its
own to work around. Unlike labwc (Openbox's look and XML config) or Wayfire (Compiz-style
plugins), it is built around today's shells and a single TOML file.

**Checked against the shells, not only in theory:** `scripts/shells` starts Noctalia and Dux
Shell on a headless Pila and walks their IPC through every feature that depends on the
compositor (workspaces, taskbar, night light, screenshots, lock, monitors, ...), after every
change to a protocol they use and before each release.

## Never a missed refresh

Smoothness is a rule, not a goal: Pila never makes a monitor miss a refresh while something
moves (an animation, a drag, an app's new frame), at the monitor's own rate, 60 Hz or 144 Hz.

- **A budget:** at most 4 ms of CPU per frame and monitor, on a laptop's integrated GPU.
- **Measured, not guessed:** `RUST_LOG=pila::frames=debug` logs per monitor the frames, their
  average and worst cost and every missed refresh, exact from the kernel's vblank counter on a
  TTY. `scripts/check` runs real programs on your hardware for a minute and reports them.
- **On demand:** nothing is drawn when nothing changed, and a change redraws only the monitors
  it shows on. An idle desktop costs nothing.
- **Nothing blocks the event loop:** fonts, the cursor theme and D-Bus load on threads; one-time
  costs (shaders, workspace slide textures) are paid at startup, not in the first frame that
  needs them.
- **Fullscreen apps go straight to the screen** (direct scanout): a fullscreen video or game is
  shown as the app drew it, with no frame of Pila's in between.

## Standard protocols first

Pila implements the official `wayland-protocols` (stable, staging, `ext-`) before any vendor
protocol, and implements a standard one itself when Smithay doesn't have it yet
(`ext-image-copy-capture`, `ext-background-effect`, ...). The wlroots protocols that panels
still need and that have no standard replacement are supported, each to be replaced as soon as
an `ext-` version exists. No KDE, GNOME or Hyprland protocols.

That gives apps a lot of what's new in Wayland:

- **Apps get their windows back** (xdg-session-management): place, size, maximized or
  fullscreen, even after Pila restarts.
- **Blur behind panels and translucent windows** that ask for it (ext-background-effect), or by
  rule.
- **Dialogs** that stay above their window and modal ones that keep the focus (xdg-dialog),
  window rules by tag (xdg-toplevel-tag).
- **Games and Vulkan:** FIFO presentation and commit timing (wp-fifo, wp-commit-timing),
  explicit sync (linux-drm-syncobj), adaptive sync, pointer lock and confinement.
- **Workspaces** that bars can show and switch (ext-workspace), a clipboard for clipboard
  managers (ext-data-control), screenshots and screen sharing (ext-image-copy-capture and
  wlr-screencopy).

## Sandboxed apps can't spy

Apps that connect through wp-security-context (Flatpak) don't see any protocol that reads the
screen, controls other windows, input or monitors, or changes the session: screen capture,
clipboard managers, taskbar control, layer shell, session lock, gamma, monitor settings,
virtual keyboards, workspaces, idle notification. Any new protocol of that kind is hidden from
them too, by policy.

## A configuration you can trust

- **One TOML file, applied as you save it**, keyboard layout and monitors included.
- **A bad file never breaks your session:** Pila keeps the running configuration and shows a
  notification that says which line is wrong and why.
- **`pila --check-config`** checks a file, the keyboard layout too, before you use it.
- **Window and layer rules** by app id, title or tag: workspace, size, position, maximized or
  fullscreen, always on top, title bar, corner radius, opacity, blur, shadow and animations per
  app, and per panel or launcher.
- **Everything documented:** a commented example with every option, a user manual organized by
  task and a man page. Tests check that the manual and man page list every default binding and
  every command-line option.

## Small touches

- Tap Mod alone to open the launcher; Alt+Tab and Mod+Tab by recent use.
- Shake the mouse to find the pointer; the cursor grows and stays sharp.
- Three- and four-finger swipes for windows and workspaces.
- Windows asking for attention get an amber title bar; a visual bell.
- Short, subtle animations, only where they help, and easy to turn off.
- The focus goes back to the window you were using when a launcher, dialog or window goes away.

## Scriptable

`pila msg` lists windows, monitors, workspaces and keyboard layouts as text or JSON, runs any
action a key can run, and with `subscribe` streams events as they happen, for bars and scripts
([IPC.md](IPC.md)).

## Any distro, no systemd required

Pila doesn't assume a distro, init system or display manager. It runs with logind, elogind or
seatd, from any display manager or a TTY, and degrades when optional tools (systemd-cat,
dbus-run-session, a journal) are missing. There are packages for Arch (AUR), Debian, Ubuntu,
Fedora and Nix, and testers run it on Arch, Debian, NixOS and in virtual machines.

## Reliable by construction

- **Every feature has a test:** an integration suite drives real Wayland clients against a
  headless Pila, and [`tests/FEATURES.md`](../tests/FEATURES.md) lists each feature with its
  tests (a test checks the list can't fall behind).
- **Bugs are reproduced first:** a fix comes with the test that failed without it.
- **Fuzzing** of the configuration, IPC and EDID parsers, and of apps sending Pila absurd
  requests (sizes, scales, buffers, popups past what's sane).
- **The real backend in a CI job:** the DRM/KMS backend runs on the kernel's virtual KMS
  driver, with monitors and a second GPU plugged in and out.
- **The lock screen is airtight by construction:** while locked, nothing but the lock surface
  can receive input, and a code path that tries fails loudly in debug builds.
- **No panic past startup** but on an invariant the code states; Xwayland is restarted if it
  dies, and a monitor that fails doesn't freeze the others.

## Under the hood

How Pila keeps those promises. Every point is described in detail in
[ARCHITECTURE.md](ARCHITECTURE.md).

### Drawing only what changed

- **Redraws per monitor, by what they bring:** a change asks for a redraw only on the monitors
  it shows on, and each monitor's frame leaves out the windows that don't reach it. Requests
  say what they bring: a change of Pila's own (a window moved, an animation) is drawn at once;
  an app's new picture too, but a redraw that finds nothing new makes the next wait until half
  a refresh before the vblank.
- **A looping app can't make Pila spin:** an app that commits nothing new in a loop of frame
  callbacks or FIFO barriers gets at most one redraw per refresh, and covered or hidden windows
  get their frame callbacks once a second instead of at the screen's rate.
- **Partial repaints:** damage tracking per monitor, per capture and per blurred region, so a
  blinking cursor in a terminal redraws a few pixels, not the screen.
- **Blur that costs only when it changes:** dual-Kawase blur, rendered from only the windows
  and panels that reach the blurred region, through a damage tracker of its own; the passes
  run again only when what's behind changed. A window under an opaque one isn't blurred at all.
- **Text and title bars cached:** fonts are shaped and rasterized on a thread, title bars kept
  as textures through resizes and focus changes.

### Apps can't slow Pila down

- **A slow app delays only its own frame:** a commit whose buffer the app's GPU is still
  writing waits for its fence (or its explicit-sync acquire point) before it applies, so Pila
  never stalls a frame on it.
- **FIFO and commit timing done right:** `wp-fifo` barriers are signaled by the monitor's
  vblank (one content update per refresh), `wp-commit-timing` timestamps are applied for the
  frame that will show at that time, from the kernel's own vblank timestamps.
- **Buffers that can go straight to the screen:** a fullscreen app is told the formats the
  monitor's primary plane takes (per-surface dmabuf feedback), so its next buffers are ones
  that can be scanned out with nothing drawn.
- **Hostile sizes are refused:** every size and position an app gives (buffers, subsurfaces,
  viewports, popups, panels, margins) is checked against a limit before anything adds them up,
  and the protocol requests are fuzzed against a running Pila.

### Screen capture without a second frame

- **Copied from the frame just shown:** on a TTY, a screen share or screenshot of a monitor is
  a GPU blit of the frame Pila just composited for it, not the scene drawn a second time.
- **Straight into the app's buffer:** captures render into the recorder's dmabuf (zero copy),
  and only when the captured area changed; the GPU's work is waited for on a fence in the
  background, never on the event loop.
- **Sound and picture in step:** each captured frame carries the time it shows on screen.
- **Shared windows keep drawing when hidden:** a shared window that's covered, minimized or on
  another workspace gets frame callbacks at the recorder's rate, so the share doesn't freeze.

### Bare metal that recovers

- **One broken monitor doesn't stop the others:** a frame that fails at the DRM level blocks
  only its own monitor, which retries with a backoff.
- **Modes that fit:** a mode beyond what the link can carry falls back to a lower refresh rate
  or a smaller size; a DisplayPort link that failed training (after a resume, through a dock)
  is retrained.
- **Monitors unplugged safely:** a monitor's `wl_output` is retired gradually, so apps binding
  it as it goes don't hit a protocol error.
- **Locked means locked on screen:** the lock is confirmed to the locker only once every
  monitor has shown a locked frame at its vblank, so nothing flashes before it.
- **Two GPUs** (`other-gpus = true`): monitors on the second GPU are drawn by the primary one
  and copied through a dmabuf, or through memory when it can't import it.
- **Exact frame accounting:** missed refreshes are counted from the kernel's vblank sequence,
  and each late frame says why: held back, drawn late or shown late.

## Built on Smithay, in Rust

Like niri and COSMIC, Pila is written in Rust on [Smithay](https://github.com/Smithay/smithay),
following its main branch, so new protocols and fixes arrive early.

## Not there yet

Pila is alpha. It doesn't support touchscreens, drawing tablets or input methods (IME) yet;
NVIDIA has never been tried; monitors wired to a second GPU are off by default and untried
(`other-gpus = true`). See [Status](../README.md#status) and, if you can, [help test
it](../README.md#help-test-pila).
