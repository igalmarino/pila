<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/igalmarino/pila/main/docs/logo/pila-logo-dark.svg">
    <img src="https://raw.githubusercontent.com/igalmarino/pila/main/docs/logo/pila-logo.svg" alt="Pila" width="420">
  </picture>
</p>

# Pila user manual

Pila is a stacking Wayland compositor: windows float, overlap, and move freely, just like on a
traditional desktop. Pila handles windows, title bars, shadows, the window switcher, the overview,
monitors, and input devices. Everything else (the bar, launcher, notifications, wallpaper, and lock
screen) is handled by separate Wayland programs (`waybar`, `fuzzel`, `mako`, `swaybg`, `swayidle`,
and `swaylock` by default when installed). The `pila-desktop` package installs all of these
together, while `pila` installs only the compositor. Full layer-shell desktop shells work too.

This manual covers daily usage and configuration. For a full list of options, see
[`config.example.toml`](https://github.com/igalmarino/pila/blob/main/config.example.toml) (also
installed in `/usr/share/doc/pila` by distro packages). For CLI flags, run `man pila`. For
installation instructions, see
[INSTALL.md](https://github.com/igalmarino/pila/blob/main/docs/INSTALL.md).

- [Starting and leaving Pila](#starting-and-leaving-pila)
- [The Mod key and default bindings](#the-mod-key-and-default-bindings)
- [Windows](#windows)
- [Looks](#looks)
- [Workspaces](#workspaces)
- [Monitors](#monitors)
- [Keyboard, mouse, and touchpad](#keyboard-mouse-and-touchpad)
- [Input methods (Chinese, Japanese, Korean, ...)](#input-methods-chinese-japanese-korean-)
- [The configuration file](#the-configuration-file)
- [Building your desktop](#building-your-desktop)
- [Screenshots and screen sharing](#screenshots-and-screen-sharing)
- [Portals, polkit, and the keyring](#portals-polkit-and-the-keyring)
- [Locking the screen and idling](#locking-the-screen-and-idling)
- [Games and X11 apps](#games-and-x11-apps)
- [Window and layer rules](#window-and-layer-rules)
- [Scripting with `pila msg`](#scripting-with-pila-msg)
- [Troubleshooting](#troubleshooting)

## Starting and leaving Pila

- **From your display manager:** Choose *Pila* in the session menu (on GDM, click the gear icon on
  the password screen). This is the standard way to run Pila.
- **From a TTY:** Log in on a virtual console and run `pila-session`. This starts Pila the same way
  a login manager does and writes logs to `~/.local/state/pila/pila.log`.
- **Nested:** Run `pila` from inside another desktop to open it in a normal window. Nothing else on
  your system changes, making it a safe way to test config edits. In nested mode, Mod defaults to
  **Alt** (so your host desktop keeps Super), and the host desktop remains in charge of your mouse,
  touchpad, and monitor settings.

At startup, Pila runs the commands in `autostart` (by default: wallpaper, `waybar`, `mako`, and
`swayidle`). No terminal opens automatically; press `Mod+Return` whenever you want one. On the very
first run, when Pila creates your default config file, it displays a short welcome overlay with
essential shortcuts (terminal, launcher, window switching, close, and quit). Any key press or click
dismisses it, and if you press a bound shortcut like `Mod+Return`, the shortcut still runs. Press
**Mod+Shift+/** (`Mod+?`) at any time to open the full shortcuts cheatsheet, reflecting your
current bindings.

**Under systemd:** `pila-session` starts Pila as the user service `pila.service`, tied to
`graphical-session.target` just like GNOME and KDE. Once Pila is ready, systemd starts your enabled
graphical user services (`WantedBy=graphical-session.target`) along with XDG autostart entries from
`~/.config/autostart` and `/etc/xdg/autostart` (respecting `OnlyShowIn`), and stops them when Pila
exits. Avoid listing the same program in both XDG autostart and Pila's `autostart`, or it will
launch twice. Under systemd, logs are also available via `journalctl --user -u pila`, and starting
a second Pila session while one is running is prevented. Set `PILA_NO_SYSTEMD=1` to run Pila
directly instead of through systemd, matching non-systemd behavior.

**Exiting:** Press **Mod+Shift+E** to quit Pila and return to the display manager or TTY. If the
screen ever freezes, press **Ctrl+Alt+F1** through **F12** to switch to another TTY, log in, and
run `pkill pila`.

## The Mod key and default bindings

Most shortcuts use the **Mod** key, which defaults to **Super** (the Windows/Command key) on a TTY
or login session, and **Alt** when running nested. Set `mod` in your config to `super`, `alt`, or
`ctrl` to override this.

| Keys | Action |
|---|---|
| `Mod+Return` | Open a terminal (`terminal`, or auto-detect `kitty`, `foot`, `alacritty`, etc.) |
| `Mod+D` | Open the launcher (`launcher`, default `fuzzel`) |
| `Mod` | Tap and release alone (with no other key or click) to open the launcher |
| `Mod+Tab`, `Mod+J` | Switch to the previously focused window; hold `Mod` and tap again to step further back |
| `Mod+Shift+Tab` | Switch windows in reverse order |
| `Alt+Tab`, `Alt+Shift+Tab` | Same as `Mod+Tab` / `Mod+Shift+Tab`, regardless of what `mod` is set to |
| `Mod+W` | Toggle the overview (spreads workspace windows out; select with clicks or arrow keys + Enter; Escape closes) |
| `Mod+M` | Toggle maximize |
| `Mod+N` | Minimize the focused window |
| `Mod+F` | Toggle fullscreen |
| `Mod+Left`, `Mod+Right` | Snap to the left or right half (or move a quarter-snapped window to the adjacent quarter); pressing again toward the same edge restores |
| `Mod+Up` | Maximize a floating window or top quarter; snap a half to its top quarter; expand a bottom quarter back to a half |
| `Mod+Down` | Restore a maximized window or bottom quarter; snap a half to its bottom quarter; expand a top quarter back to a half |
| `Mod+Shift+Left`, `Mod+Shift+Right`, `Mod+Shift+Up`, `Mod+Shift+Down` | Move the focused window by 40 px |
| `Mod+Ctrl+Right`, `Mod+Ctrl+Down` | Grow window width / height |
| `Mod+Ctrl+Left`, `Mod+Ctrl+Up` | Shrink window width / height |
| `Mod+C` | Center the focused window |
| `Mod+T` | Toggle always-on-top |
| `Mod+P` | Toggle visible on all workspaces (sticky) |
| `Mod+H` | Minimize all windows on the current workspace (show desktop); press again to restore |
| `Mod+O` | Move pointer and focus to the next monitor |
| `Mod+Shift+O` | Move the focused window to the next monitor |
| `Mod+1` to `Mod+9` | Switch to workspace 1–9 |
| `Mod+Shift+1` to `Mod+Shift+9` | Move the focused window to workspace 1–9 |
| `Mod+Page_Down`, `Mod+Page_Up` | Next / previous workspace |
| `Mod+Shift+Page_Down`, `Mod+Shift+Page_Up` | Move the focused window to the next / previous workspace |
| `Mod+WheelScrollDown`, `Mod+WheelScrollUp` | Next / previous workspace with the mouse wheel (cooldown prevents overshooting on fast spins) |
| `Mod+Shift+WheelScrollDown`, `Mod+Shift+WheelScrollUp` | Move the focused window to the next / previous workspace |
| `Mod+Shift+S` | Capture a selected screen region to the clipboard (requires `grim`, `slurp`, `wl-clipboard`) |
| `Mod+Ctrl+S` | Save a screenshot of the monitor under the pointer to `~/Pictures` (requires `grim`) |
| `XF86AudioRaiseVolume`, `XF86AudioLowerVolume` | Adjust volume ±5% (capped at 100%, repeats while held; requires `wpctl`) |
| `XF86AudioMute`, `XF86AudioMicMute` | Toggle speaker or microphone mute (requires `wpctl`) |
| `XF86MonBrightnessUp`, `XF86MonBrightnessDown` | Adjust screen brightness ±5% (repeats while held, won't turn backlight completely off; requires `brightnessctl`) |
| `XF86AudioPlay`, `XF86AudioPause`, `XF86AudioNext`, `XF86AudioPrev`, `XF86AudioStop` | Control media playback (requires `playerctl`) |
| `Mod+Shift+Q` | Close the focused window |
| `Mod+Shift+E` | Quit Pila |
| `Mod+Shift+slash` | `Mod+?`: toggle the shortcuts cheatsheet (any key or click dismisses it) |
| `Mod` + left drag | Move a window from anywhere inside it |
| `Mod` + right drag | Resize a window from its nearest corner |
| `Ctrl+Alt+F1` to `F12` | Switch to another virtual terminal (bare metal only) |

**Volume, brightness, and media keys:** By default these call `wpctl` (PipeWire/WirePlumber),
`brightnessctl`, and `playerctl`. If a tool isn't installed, the key does nothing and Pila shows a
one-time notification telling you which program is missing. On PulseAudio, rebind the volume keys to
`pactl`. Pila doesn't draw an on-screen volume/brightness indicator itself; if you want one, rebind
the keys to a tool like `swayosd` (e.g., `swayosd-client --output-volume raise`).

Every binding except the two `Mod` drags (which follow the `mod` option) and VT switching can be
rebound or disabled under `[bindings]` (see [The configuration file](#the-configuration-file)).
Touchpad gestures are covered in [Keyboard, mouse, and touchpad](#keyboard-mouse-and-touchpad).

## Windows

**Mouse controls.** Apps that accept server-side decorations get a Pila title bar with minimize,
maximize, and close buttons: drag the title bar to move and drag any edge or corner to resize. Apps
that draw their own title bars (GTK apps, browsers) keep their native behavior. `Mod` + left drag
moves any window from anywhere inside it, and `Mod` + right drag resizes from the nearest corner.
Right-clicking any title bar (Pila's or an app's own) opens the window menu (minimize, maximize,
always on top, all workspaces, close), navigable with Up/Down/Enter and closed with Escape or a
click outside.

**Snapping.** Drag a window to the left or right screen edge to snap it to that half, near a corner
to snap it to a quarter, or to the top edge to maximize it. A translucent preview highlights the
target area before you release the button.

With the keyboard, `Mod` + arrow keys (`snap <left|right|up|down>`) step smoothly between states:
- `Mod+Left` snaps to the left half; pressing `Mod+Up` or `Mod+Down` then narrows it to the top or
  bottom quarter.
- `Mod+Right` shifts a left quarter over to the right quarter; pressing the opposite vertical arrow
  expands a quarter back into a half.
- `Mod+Up` maximizes a floating window or a top quarter; `Mod+Down` restores a maximized window or
  a bottom quarter.
- Snapped windows remember their floating size and position. Restore them by pressing the arrow
  toward the edge they're already against, pressing `Mod+Down` past the bottom, running `restore`,
  or simply dragging the window away.

Direct snap actions also exist if you prefer fixed bindings: `snap-left`, `snap-right`, `snap-top`,
`snap-bottom` (handy on vertical monitors), `snap-top-left`, `snap-top-right`, `snap-bottom-left`,
and `snap-bottom-right` (repeating the same action restores). For simpler halves-and-maximize arrow
bindings:

```toml
[bindings]
"Mod+Left" = "snap-left"
"Mod+Right" = "snap-right"
"Mod+Up" = "maximize"
"Mod+Down" = "restore"
```

**Maximize and fullscreen.** `Mod+M` and `Mod+Up` maximize the focused window (filling the monitor
while leaving space for panels); `Mod+F` toggles fullscreen. Press again to restore. By default,
maximized and fullscreen windows have square corners and no shadow.

**Gaps.** Setting `gaps` adds spacing (in logical pixels) around maximized windows and around and
between snapped halves and quarters. When `gaps` is non-zero, maximized windows keep their rounded
corners and shadow like floating windows; fullscreen windows still cover the full monitor. Default
is `0`:

```toml
gaps = 12
```

**Keyboard movement and resizing.** `Mod+Shift` + arrows moves the focused window, `Mod+Ctrl` +
arrows resizes it (right/down grow, left/up shrink), and `Mod+C` centers it.

**Switching windows.** A quick `Mod+Tab` toggles between the two most recently used windows. Hold
`Mod` and tap `Tab` repeatedly to cycle further back; each window raises as you reach it, and
releasing `Mod` confirms the selection. Hold `Mod` for a moment to bring up the visual window
switcher centered on the monitor, showing live previews (videos keep playing) and titles in
most-recently-used order. You can also click any preview directly. `Mod+Shift+Tab` cycles in
reverse.

`Alt+Tab` and `Alt+Shift+Tab` mirror `Mod+Tab` and `Mod+Shift+Tab` regardless of your `mod`
setting, matching other desktops. Because Pila intercepts them, apps won't see `Alt+Tab` unless you
unbind them with `"Alt+Tab" = "none"` and `"Alt+Shift+Tab" = "none"` under `[bindings]`.

**The overview.** Open the overview with `Mod+W`, a four-finger swipe up, or by pushing the pointer
into the top-left corner of the screen (the hot corner; disabled over fullscreen windows and while a
button is held). It spreads all windows on the active workspace (including minimized
ones) across their respective monitors as live previews, scaled down so none overlap, over a dimmed
wallpaper while keeping your bar visible.

- Click a window, or select it with the arrow keys (or pointer) and press Enter, to focus it.
- Close the overview with Escape, `Mod+W`, the hot corner, a four-finger swipe down, or by clicking
  empty space.
- The hot corner is set under `[overview]`: `hot-corner-position` picks the corner (`top-left`,
  `top-right`, `bottom-left` or `bottom-right`; on several monitors, only a corner of the whole
  layout counts), and two settings keep it from firing by accident: `hot-corner-pressure` (logical
  pixels the pointer must be pushed past the corner, with a mouse or touchpad; try 100) and
  `hot-corner-delay` (milliseconds it must stay there; try 250). With both, the delay counts from
  reaching the corner, not from the push: a push that takes longer than the delay opens it as soon
  as it's far enough. `hot-corner = false` turns it off.
- Other keybindings close the overview and run normally, except workspace-switching bindings
  (`Mod+1`–`Mod+9`, etc.), which switch workspaces while keeping the overview open.
- When you have more than one workspace, a live workspace strip appears at the top of each monitor
  with the active workspace highlighted in the accent color. Click a workspace to switch to it, or
  drag any window onto a workspace thumbnail to move it there (dropping it anywhere else returns it
  to its spot).

**Urgent windows.** When a background app requests attention (for example, a new chat message or
completed download), its title bar turns amber (`[colors] bar-urgent`) and compatible bars flag its
workspace. Focusing the window clears the urgent state.

**Minimizing.** `Mod+N` or the title bar minimize button hides a window. Restore it through
`Mod+Tab`, the overview, or your taskbar. `Mod+H` minimizes all windows on the current workspace
(show desktop); press `Mod+H` again to restore them.

**Always on top and all workspaces.** `Mod+T` keeps a window above all others (even over fullscreen
windows). `Mod+P` pins a window to every workspace. Both toggle off when pressed again and can also
be set via window rules.

**Closing windows.** `Mod+Shift+Q` or the close button sends a close request, giving the app a
chance to prompt about unsaved work.

**Session restore.** Apps supporting `xdg-session-management` automatically get their previous
window positions, sizes, and maximized/fullscreen states back when reopened, even after Pila
restarts. State is stored in `~/.local/state/pila/sessions.json`. Nested instances and secondary
Pila sessions keep this state in memory only.

## Looks

Everything Pila renders itself (title bars, shadows, rounded corners, and animations) is configured
in `config.toml` and updates live when you save.

**Title bars and colors.** The `[colors]` section styles Pila's title bars: focused (`bar`),
unfocused (`bar-inactive`), and urgent (`bar-urgent`); title text and button icons (`icon`,
`icon-inactive`); button hover states (`hover`, `hover-icon`, `close-hover`, `close-hover-icon`);
the fallback desktop `background`; and the drag-snap highlight (`snap-preview`, which supports
alpha as `#rrggbbaa`). Client-side decorated apps keep their own colors. Pila's window switcher,
overview, and portal panels automatically inherit `[colors]`, and external tools can read them via
`pila msg colors`.

**Wallpaper.** Pila delegates wallpaper rendering to `swaybg` (or your desktop shell). By default,
`autostart` runs bare `swaybg` and Pila passes it a generated minimalist wallpaper matching your
`[colors]`: solid `background` with a subtle three-window stack emblem in the bottom-right corner
accented with `snap-preview`. It regenerates automatically whenever `[colors]` changes (applied on
the next `swaybg` start) and scales cleanly to any aspect ratio (`-m fit`).

To use your own image, replace `"swaybg"` in `autostart` with a `swaybg` command specifying `-i`
and a scaling mode (`fill`, `fit`, `stretch`, `center`, or `tile`):

```toml
autostart = [
    "swaybg -i ~/Pictures/wallpaper.jpg -m fill",
    "waybar",
    "mako",
    "swayidle -w timeout 300 'swaylock -f' timeout 600 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
]
```

Because `autostart` only runs at login, apply a new wallpaper immediately by running `pkill swaybg`
and starting it again (`swaybg -i FILE -m fill &`). Without `swaybg`, Pila displays `[colors]
background`. Legacy configs containing `[wallpaper]` still load with a warning showing the
equivalent `swaybg` command.

**Rounded corners.** By default, Pila rounds the top two corners by 8 logical pixels and leaves the
bottom corners square, matching GTK windows. Set `corner-radius` from `0` (square) to `64` (clamped
to half the window's shorter side, or the 28 px title bar height when decorated), and set
`rounded-corners = "all"` to round all four corners instead of `"top"`. Maximized and fullscreen
windows stay square (unless `gaps` is set), and apps that draw their own shadows (such as GTK)
handle their own rounding so they don't get double corners. You can override the radius per app in
a window rule. Shadows, background blur, bell flashes, and snap previews all follow the window's
corner shape.

**Shadows.** Windows cast a drop shadow that is darker under the focused window. Maximized windows
(with `gaps = 0`), fullscreen windows, and apps that draw their own shadows (GTK) don't get an
extra Pila shadow. Configure `[shadow]` with `enabled`, `size` (0–64), `offset`, `color`, and
`inactive-color` (`#rrggbbaa`), or override per app with a window rule's `shadow` field.

**Animations.** Opening, closing, minimizing, maximizing, snapping, fullscreen transitions, layer
surfaces (launchers/notifications), workspace slides, and the overview all have short animations
configured under `[animations]`:

```toml
[animations]
speed = 2.0             # scale all durations (0.25 to 4; 2.0 = half speed)
window-close = false    # disable closing animation
workspace = 150         # workspace slide duration in ms (1 to 1000)
# enabled = false       # disable all animations
```

Window and layer rules can disable animations for specific apps or shells (`animations = false` or
`animations = { open = false }`). If you run a desktop shell that animates its own panels, disable
Pila's layer animations for it (see [Building your desktop](#building-your-desktop)).

**Blur and transparency.** Apps that request background blur via `ext-background-effect` (such as
certain terminals, shell panels, and `fcitx5`'s candidate window) get blurred automatically,
including behind their popups and menus. In window or layer rules:
- `blur = true` forces blur behind a window (including its title bar) or panel that doesn't request
  it; `blur = false` blocks blur requests for that window and its popups.
- `popup-blur = true` forces blur behind menus and popups that don't request it (such as GTK
  menus), matching the menu's rounded corners and popover arrows when provided. Otherwise, set
  `popup-corner-radius` to match its rounding. `popup-blur = false` blocks popup blur requests.
- Panel blur applies to panels drawn above windows (`top` and `overlay` layers).
- `opacity` (`0.1` to `1.0`) makes an entire window and its menus translucent.

See [Window and layer rules](#window-and-layer-rules) for examples.

**System bell.** When an app rings the system bell (`xdg-system-bell`), Pila briefly flashes its
window; no sound is played.

## Workspaces

Pila starts with 4 workspaces by default (configurable from 1 to 32 via `workspaces`). Workspaces
span all monitors, so switching workspaces updates every monitor together. Windows pinned with
`Mod+P` remain visible across all workspaces.

- `Mod+1`–`Mod+9` switch workspaces; `Mod+Shift+1`–`Mod+Shift+9` move the focused window. Pila only
  grabs digits up to your configured workspace count; unused digits pass through to apps.
- `Mod+Page_Down` / `Mod+Page_Up` step to the next or previous workspace (stopping at the ends).
  `Mod` + mouse wheel does the same, and `Mod+Shift` + wheel moves the focused window. Scroll
  notches within 150 ms of the last switch are ignored so a fast flick advances one workspace at a
  time; adjust this with `[input] wheel-binding-cooldown` (`0` to `1000` ms, or `0` to disable).
- On a touchpad, swipe left or right with four fingers to switch workspaces.
- Bars and pagers supporting `ext-workspace` (like waybar's `ext/workspaces` module) display and
  switch workspaces directly.

Use a window rule's `open-on-workspace` option to assign specific apps to a workspace automatically.

## Monitors

Pila detects all connected monitors and supports hotplugging while running. Unconfigured monitors
use their preferred mode at scale `1`, placed to the right of existing outputs. If a preferred mode
fails (for instance, two 4K displays exceeding a dock's bandwidth), Pila automatically falls back
to lower refresh rates at the same resolution, then smaller resolutions, logging the mode chosen.

Use `[[output]]` blocks in `config.toml` to configure resolution and refresh rate (`mode`), scaling
(including fractional scales like `1.25`), `position`, rotation (`transform`), variable refresh
rate (`adaptive-sync`), or to disable a monitor (`enabled = false`). Match monitors either by
connector (`eDP-1`, `DP-1`, `HDMI-A-1`) or by make, model, and optional serial number so settings
follow the display regardless of which port it's plugged into. Run `wlr-randr` or check Pila's log
to see connected monitor names, makes, and models.

Runtime tools (`wlr-randr`, desktop display settings) can adjust monitors on the fly; their changes
remain in effect until you edit that monitor's `[[output]]` block in `config.toml`.

**Monitors on a second GPU (experimental).** On dual-GPU laptops, HDMI or DisplayPort ports are
often wired to the discrete GPU, and some docks contain their own GPU. Pila leaves secondary-GPU
outputs disabled by default because they haven't yet been tested on real multi-GPU hardware. Set
`other-gpus = true` to enable them. Pila renders on the primary GPU and copies frames to the
secondary GPU for scanout. Toggling this takes effect immediately on save, and `pila-report` lists
which GPUs and outputs are active. Test reports on multi-GPU setups are very welcome.

**Switching monitors:** `Mod+O` moves pointer and focus to the next monitor; `Mod+Shift+O` moves
the focused window there, preserving its relative placement, maximized state, or snapped side.

**Monitor power:** By default, `swayidle` turns monitors off after 10 minutes of inactivity (see
[Locking the screen and idling](#locking-the-screen-and-idling)). To turn monitors off manually,
run `pila msg action power-off-monitors` (or bind it to a key). Any key press, click, or pointer
movement wakes them, and that waking key or click is consumed so it won't accidentally trigger
anything. Via `pila msg`, `power-off-monitors` also works while the screen is locked (keybindings
are disabled while locked). `pila msg action power-on-monitors` wakes monitors from a script.
`wlopm --off '*'` and `wlopm --on '*'` work too, though monitors powered off via `wlopm` stay off
until `wlopm` turns them back on. `pila msg outputs` tags powered-off monitors with `[off]`.

## Keyboard, mouse, and touchpad

**Keyboard layout.** Configure XKB `layout`, `variant`, `model`, and `options` under `[keyboard]`
(on systemd distros, `localectl list-x11-keymap-layouts` lists available layouts). Example with
two layouts and a toggle key:

```toml
[keyboard]
layout = "us,es"
options = "grp:alt_shift_toggle"
```

You can also switch layouts via keybindings or IPC using `switch-layout next`, `switch-layout prev`,
or a 1-based index matching `layout` (`pila msg action switch-layout next`). Query active layouts
with `pila msg keyboard` or stream changes with `pila msg subscribe keyboard`.

`repeat-delay` and `repeat-rate` control key repeat timing. Setting `numlock = true` enables Num
Lock at startup and re-enables it whenever a reloaded `[keyboard]` section loads a new keymap. Run
`pila --check-config` to catch invalid XKB layout names before saving.

**Mouse and touchpad.** `[input.touchpad]` and `[input.mouse]` configure natural scrolling, pointer
speed and acceleration profile, tap-to-click (enabled by default), disable-while-typing, clickpad
click method, and middle-click emulation. Omitted options keep libinput's device defaults. Mouse
wheel bindings work with modifiers (such as `Mod` + wheel for workspaces, controlled by
`wheel-binding-cooldown`).

**Touchpad gestures.** Default swipes handled by Pila:

| Gesture | Action |
|---|---|
| `swipe-3-left`, `swipe-3-right` | Switch windows (like `Mod+Tab` / `Mod+Shift+Tab`); keep swiping to step further |
| `swipe-3-up` | Toggle maximize |
| `swipe-3-down` | Restore a snapped or maximized window |
| `swipe-4-left`, `swipe-4-right` | Next / previous workspace |
| `swipe-4-up`, `swipe-4-down` | Open / close the overview |

Customize 3-, 4-, and 5-finger swipes under `[gestures]` with any bindable action. Pinch gestures
and unbound finger counts pass straight through to apps.

**Cursor theme and shake-to-find.** Shake the mouse (or wiggle a finger back and forth quickly on a
touchpad) to temporarily enlarge the cursor up to 3x so you can spot it on large or multi-monitor
setups. Straight lines and circles won't trigger it. Configure or disable under `[cursor]`:

```toml
[cursor]
shake-magnification = 2.0   # 1.5 to 5
# shake-to-find = false     # disable cursor magnification
```

Set `theme` and `size` under `[cursor]` to override `XCURSOR_THEME` and `XCURSOR_SIZE` (defaults:
`"default"` and `24`). Changes apply immediately on save and are exported as `XCURSOR_THEME` and
`XCURSOR_SIZE` to newly launched apps (already-running X11 and legacy Wayland apps keep their old
cursor until restarted, while `cursor-shape` clients like GTK 4, Qt, `foot`, and `kitty` update
automatically). Pila picks larger bitmap sizes from the cursor theme while magnifying so the
pointer stays sharp. `[animations] speed` scales the grow/shrink animation (instant when animations
are off).

Note: mouse, touchpad, and gesture settings only apply on bare metal (login session or TTY); in
nested mode, the host desktop handles pointing devices, while `[keyboard]` settings apply in both.

## Input methods (Chinese, Japanese, Korean, ...)

Pila supports Wayland input methods speaking `input-method-v2` (such as `fcitx5`) for typing CJK
and other complex scripts. Launch your input method from `autostart`:

```toml
autostart = ["fcitx5 -d", "waybar", "kitty"]
```

Native Wayland apps (GTK 3/4, Qt 6.7+, `foot`, `kitty`) talk to it automatically via
`text-input-v3` with no extra setup. Do **not** set `GTK_IM_MODULE` or `QT_IM_MODULE`, as those tell
toolkits to bypass Pila and talk directly to `fcitx5` over D-Bus.

- **Chromium and Electron apps:** pass `--enable-wayland-ime --wayland-text-input-version=3`.
- **X11 apps (via Xwayland):** use XIM by adding `XMODIFIERS = "@im=fcitx"` under `[environment]`
  (see [Building your desktop](#building-your-desktop)) or in your login environment.

Pila's keybindings always take priority over the input method. Candidate popups position themselves
below the text cursor (or above if space is tight), staying clear of panels. Input methods are
automatically disabled on the lock screen so passwords never pass through them.

## The configuration file

Pila reads `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila/config.toml`). On first launch,
if no config file exists, Pila copies the built-in `config.example.toml` there. Every uncommented
line in `config.example.toml` is an active default; commented lines are examples or machine-specific
settings (`mod`, `terminal`, `[keyboard]`); uncomment a line to enable it.

Because Pila embeds `config.example.toml` as its fallback, any option you delete or omit from your
config automatically uses the current default. Options you explicitly keep stay fixed across
upgrades, while omitted ones pick up any future default improvements.

Upgrading Pila never overwrites your config file. To compare your config against the latest example
(installed in `/usr/share/doc/pila` by distro packages) or reset it:

```bash
diff ~/.config/pila/config.toml /usr/share/doc/pila/config.example.toml
cp /usr/share/doc/pila/config.example.toml ~/.config/pila/config.toml
```

**Live reload:** Saving `config.toml` applies every change immediately except `autostart` (which
only runs at startup). If your saved file has a syntax or value error, Pila keeps the active config
and shows a notification (and log entry) with the line number and reason. If the config is invalid
at startup, Pila falls back to defaults and notifies you. Valid files with likely mistakes (such as
two `[[output]]` blocks matching the same monitor, where only the first applies) load with a warning
notification.

Run `pila --check-config` to validate your config and XKB keymap without launching Pila.

**Keybindings:** Add, replace, or remove bindings under `[bindings]`:

```toml
[bindings]
"Mod+Shift+Return" = "spawn foot -e htop"
"XF86AudioRaiseVolume" = { action = "spawn pactl set-sink-volume @DEFAULT_SINK@ +5%", repeat = true }
"XF86AudioPlay" = "none"  # pass key through to apps instead
"Mod+J" = "none"          # remove a default binding
```

- Set `default-bindings = false` at the top of the file (before any `[table]`) to disable all
  built-in bindings and gestures so only your own apply, which is useful for custom layouts or running Pila
  as a `greetd` greeter compositor. (Note that the generated starter config already copies the
  defaults into its own `[bindings]` and `[gestures]` tables, so delete unwanted lines directly
  there. `Ctrl+Alt+F1..F12` VT switching is always available.)
- Bindings normally trigger once per key press. Use `{ action = "...", repeat = true }` to repeat
  while held (using `[keyboard]`'s `repeat-delay` and `repeat-rate`). Repeat applies only to
  regular keys, not scroll notches or modifier taps.
- Key names use XKB keysyms (`Return`, `Tab`, `q`, `F5`, `XF86AudioMute`), or mouse wheel
  directions (`WheelScrollUp`, `WheelScrollDown`, `WheelScrollLeft`, `WheelScrollRight`, governed
  by `[input] wheel-binding-cooldown` and never triggered by touchpad scrolling).
- Modifiers (`Mod`, `Shift`, `Ctrl`, `Alt`, `Super`) must match exactly.
- `spawn` runs its command through `sh` with `PILA_OUTPUT` set to the monitor under the pointer.
  All available actions are listed in `config.example.toml`.

The environment variables `PILA_MOD`, `PILA_TERMINAL`, and `PILA_LAUNCHER` override `mod`,
`terminal`, and `launcher`.

## Building your desktop

Pila launches everything in `autostart` once ready (including X11 apps, which trigger Xwayland
automatically). The default `autostart` runs `swaybg`, `waybar`, `mako`, and `swayidle`, with no
terminal. Set `autostart = []` to start nothing. Missing programs from the default `autostart` are
skipped quietly (logged once) so bare `pila` works cleanly without `pila-desktop`; missing programs
from your own custom commands trigger a notification.

**Standard tools, easily swapped.** Pila does not ship its own bar, launcher, or OSD. Everything is
configured through `config.toml` and can be replaced freely:

| Default | Setting | Alternatives |
|---|---|---|
| `fuzzel` | `launcher` | `wofi --show drun`, `rofi -show drun`, etc. |
| `swaybg`, `waybar`, `mako` | `autostart` | Custom `swaybg -i FILE -m fill`, another bar, or another notification daemon |
| `wpctl`, `brightnessctl`, `playerctl` | Volume/brightness/media `[bindings]` | `pactl`, `swayosd-client`, etc. |
| `grim`, `slurp`, `wl-copy` | `Mod+Shift+S` and `Mod+Ctrl+S` | Any Wayland screenshot tool |

**Matching default tool themes (`tool-configs`).** When you run the default commands unmodified
(`waybar`, `mako`, `fuzzel`, `swaybg`, and `swaylock` inside `swayidle`) and don't already have
your own config for a tool, Pila generates matching configs on the fly in
`$XDG_RUNTIME_DIR/pila-tools` (cleared on logout). They inherit your `[colors]`, system sans-serif
font (`fc-match sans-serif`), and 8 px rounded corners for floating popups (while keeping `waybar`
square along the screen edge). Pila's `waybar` config displays workspaces and window icons on the
left, and tray, volume (click to mute), battery, and clock on the right, using Font Awesome icons
when installed (`pila-desktop` includes it) and plain text labels otherwise.

Because Pila passes these configs via command-line flags (`waybar -c ...`, `mako -c ...`,
`fuzzel --config=...`, etc.) without touching `~/.config`, your existing configs and other desktop
sessions remain unaffected. To customize a generated config, copy it into `~/.config` (for example,
`cp -r $XDG_RUNTIME_DIR/pila-tools/waybar ~/.config/`) and edit it there; Pila automatically defers
to your config on the next launch. Changing `[colors]` regenerates the files for the next time each
tool starts. Set `tool-configs = false` to let every tool use its own upstream defaults.

User bindings override defaults for the same keys; `"Key" = "none"` removes a default, and
`default-bindings = false` clears all of them. Under systemd, you can also run tools as user
services (`systemctl --user enable waybar.service` instead of listing `waybar` in `autostart`) so
systemd restarts them on crash and stops them at logout.

**Environment variables.** Set environment variables for all programs Pila launches (`autostart`,
bindings, terminal, launcher) under `[environment]`, or set a variable to `false` to unset it:

```toml
[environment]
LANGUAGE = "zh_CN:zh_Hans:zh"
XMODIFIERS = "@im=fcitx"
QT_QPA_PLATFORMTHEME = false
```

Changes apply to programs started after saving. On a TTY, `[environment]` is also exported to the
D-Bus session bus and `systemd --user` so portals and D-Bus-activated apps receive them. Variables
managed by Pila itself (`WAYLAND_DISPLAY`, `DISPLAY`, `XAUTHORITY`, `XDG_CURRENT_DESKTOP`,
`XDG_SESSION_TYPE`, `PILA_SOCKET`, `PILA_OUTPUT`, plus `XCURSOR_THEME`/`XCURSOR_SIZE` when set
under `[cursor]`) cannot be overridden here and trigger a warning if listed. Pila itself doesn't
read `[environment]`.

By default, `[environment]` nudges Wayland-capable apps onto Wayland (falling back to Xwayland):
`QT_QPA_PLATFORM = "wayland;xcb"`, `ELECTRON_OZONE_PLATFORM_HINT = "auto"`,
`MOZ_ENABLE_WAYLAND = "1"`, and `_JAVA_AWT_WM_NONREPARENTING = "1"`. Override any of these with
your own value, or set to `false` to drop it (e.g., `QT_QPA_PLATFORM = false` to force Qt onto
X11). `GDK_BACKEND` and `SDL_VIDEODRIVER` are intentionally left unset: GTK already defaults to
Wayland, and older bundled SDL libraries in some games fail to launch with
`SDL_VIDEODRIVER = "wayland,x11"`.

**Using a full desktop shell.** If you prefer an all-in-one layer-shell desktop (bar, launcher,
notifications, wallpaper, lock screen), put it in `autostart` instead of `waybar` and point
`launcher` to its launcher command. If the shell animates its own panels, disable Pila's layer
animations for its namespace:

```toml
[[layer-rule]]
namespace = "my-shell-*"
animations = false
```

A few shell tips:
- Some shells call `wlr-randr --off` without a monitor name on idle, which fails. Configure their
  idle command to run `pila msg action power-off-monitors` instead, with no resume command needed
  (any input wakes the monitors).
- Logout buttons using `loginctl terminate-session` work under systemd/elogind; elsewhere, point
  logout to `pila msg action quit`.
- Shell widgets tied to another compositor's proprietary IPC (such as an overview or layout
  indicator) won't work until the shell adds Pila support, though Pila's own bindings and
  `pila msg` still work.

**Minimal separate-tools example:**

```toml
launcher = "fuzzel"
autostart = [
    "waybar",
    "mako",
    "swayidle -w timeout 300 'swaylock -f' timeout 600 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
]
```

**Window switcher:** Built into Pila. Holding `Mod+Tab` or `Alt+Tab` shows live previews styled with
your `[colors]`, while a quick tap switches immediately without flashing the overlay. External
switchers can also follow switcher state via `pila msg subscribe switch`.

**Tool compatibility:**
- **Tested and working:** `waybar` (with `ext/workspaces` and `wlr/taskbar`), `fuzzel`, `swaybg`,
  `mako`, `swaylock`, `swayidle`, `wlsunset`, `grim`, `slurp`, `wl-clipboard`, `wtype`,
  `wlr-randr`, `wlopm`, and `fcitx5`. Most are covered by automated tests or `scripts/check`.
- **Expected to work (untested):** standard `ext-data-control` clipboard managers and `screencopy`
  recorders like `wf-recorder`.
- **Partial support:** `ironbar` shows windows and the clock, but its `workspaces` module targets
  specific compositors (sway, Hyprland) and its `clipboard` module requires legacy
  `wlr-data-control`. `nwg-panel` shows its clock and controls without taskbar/workspaces.
- **Not supported:** Tools tied to another compositor's private IPC (`dwlb`, `HyprPanel`, waybar's
  `sway/*` and `hyprland/*` modules) and X11 bars (`polybar`). If a tool using standard Wayland
  protocols fails on Pila, please [open an issue](https://github.com/igalmarino/pila/issues).

**Custom Waybar example:** If you write your own `~/.config/waybar/config.jsonc`, use
`ext/workspaces` and `wlr/taskbar`:

```jsonc
{
    "layer": "top",
    "modules-left": ["ext/workspaces", "wlr/taskbar"],
    "modules-right": ["clock"],
    "ext/workspaces": {
        "format": "{name}",
        "on-click": "activate",
        "sort-by-id": true
    },
    "wlr/taskbar": {
        "format": "{icon} {title}",
        "on-click": "activate",
        "on-click-middle": "close"
    }
}
```

Highlight the active workspace in `~/.config/waybar/style.css`:

```css
#workspaces button.active { background: #4a7bd0; }
```

For things without a Wayland protocol (like keyboard layout), subscribe via `pila msg`:

```json
"custom/layout": {
    "exec": "pila msg --json subscribe keyboard | jq --unbuffered -r '.layouts[]? | select(.active) | .name'",
    "on-click": "pila msg action switch-layout next"
}
```

**Environment and notifications:** Programs Pila launches inherit `XDG_CURRENT_DESKTOP=pila` and
`PILA_SOCKET`. Install `xdg-utils` so terminals like `kitty` can open links with `xdg-open`. Pila
sends its own notifications (such as config errors) directly over D-Bus to whatever notification
daemon is running; without a daemon, messages are only written to the log.

## Screenshots and screen sharing

**Manual screenshots:** `Mod+Shift+S` selects a region with `slurp` and copies a PNG to the
clipboard (`grim`, `slurp`, `wl-clipboard`). `Mod+Ctrl+S` saves the monitor under the pointer to
`~/Pictures/screenshot-YYYYMMDD-HHMMSS.png` (`grim`). Standalone `grim` commands and shell
screenshot tools also work normally.

**App screenshots (portal):** When a browser, Flatpak, or other app requests a screenshot through
`xdg-desktop-portal`, Pila handles it directly. If the portal's permission store lets the app
capture without prompting, it receives all monitors stitched in their current layout. Otherwise,
Pila opens its picker panel with two tabs: **Screens** (monitors or a custom region) and **Windows**
(a single window, even if covered or minimized). Switch tabs with `Tab` or a click, choose with
clicks or arrow keys, then click **Take** (or press Enter) or **Cancel** (Escape). The cursor is
omitted. Images are saved under `$XDG_RUNTIME_DIR/pila-screenshots` (last 10 kept until logout) and
handed to the app.

**App global shortcuts (portal):** Apps requesting global shortcuts through the portal (such as
push-to-talk in voice chat or start/stop in a recorder) work regardless of which window is focused.
On first request, Pila opens a panel showing the app's suggested keys. Pick an entry with arrows or
clicks and press your desired shortcut (using Ctrl, Alt, or Super, or a non-typing key like F1–F12
or a media key), or press Backspace to leave it unbound, then click **Save** (Enter) or **Cancel**
(Escape). Keys already bound by Pila or another app are rejected. Choices are saved in
`~/.local/state/pila/app-shortcuts.json` (and can be edited later if the app offers a configure
button). Bound keys go exclusively to that app, not to the focused window, and are ignored while
locked or while an app inhibits shortcuts.

**Screen color picker (portal):** When an app (GTK color chooser, GIMP, Inkscape) asks the portal
to pick a color, the pointer turns into a crosshair with a top banner. Click any pixel to sample it
or press Escape to cancel; apps receive no clicks or keys while picking.

**Screen sharing (portal):** Browsers, OBS, and video call apps share the screen through
`xdg-desktop-portal`, which routes directly to Pila's built-in PipeWire backend (no
`xdg-desktop-portal-wlr` needed). When an app asks to share, Pila shows a modal picker naming the
app with live previews across two tabs:

- **Screens:** Lists all monitors (defaulting to the one under the pointer), followed by **Region**
  to share a custom rectangle. Choosing **Region** hides the picker and turns the pointer into a
  crosshair; drag a rectangle on a monitor (a top banner shows its dimensions in pixels, rounded
  down to even numbers, as video encoders need them) and release to start sharing, or press
  Escape to return to the picker. While active, a thin accent-colored outline marks the shared
  region on screen without appearing in the stream.
- **Windows** (when the app supports window sharing): Lists all windows in most-recently-used
  order, including minimized windows and those on other workspaces. A shared window streams at its
  own size and keeps updating while covered, minimized, or on another workspace; closing the window
  ends the share.

Switch tabs with `Tab` or a click (each tab remembers its selection), pick with arrows or clicks,
and confirm with **Share** (Enter) or **Cancel** (Escape). Nothing streams until you confirm,
locking the screen cancels an open picker and pauses updates to active window shares, and the
cursor is included according to the app's request. Requires `xdg-desktop-portal`, `pipewire`, and a
session D-Bus bus (set up by your login manager or `pila-session`; not available in nested mode).

**Remembering a share:** Apps that ask the portal to remember their share (recorders such as Kooha
and OBS do) get a **Remember** toggle under the picker's tiles, off by default: press `R` or click
it. With it on, the next time that app asks, it gets the same monitor, region, or window (matched
by app ID and title) again without the picker. If what was remembered isn't there (a monitor
unplugged, a window closed), the picker asks again with the toggle on; turning it off forgets the
grant. Nothing is shared while the screen is locked. Grants the app asked to keep are saved in
`~/.local/state/pila/screen-shares.json` (those meant to last while the app runs stay in memory).
List them with `pila msg screen-shares`, and revoke one with `pila msg forget-screen-share <id>`
(or `all`).

**Direct screen capture permissions:** Unsandboxed Wayland tools such as `grim`, `wf-recorder`, and
screen lockers that blur a screenshot capture the screen directly via `wlr-screencopy` or
`ext-image-copy-capture` without going through the desktop portal (sandboxed apps, such as
Flatpaks, never see these protocols). By default, any unsandboxed program is allowed. To require confirmation before an unlisted program
captures the screen, enable `[permissions]` and add `[[permission]]` rules for tools you trust:

```toml
[permissions]
enforce = true

[[permission]]
executable = "/usr/bin/grim"
access = "allow"

[[permission]]
executable = "/usr/bin/wf-recorder"
access = "allow"
```

Rules match the client's executable path with `*` wildcards; they apply in order and the last match
wins (`access` is `"allow"`, `"ask"`, or `"deny"`). When an unlisted program (or one matched by
`access = "ask"`) tries to capture, Pila opens a modal prompt showing its name and executable path.
Press `Enter` (or click **Allow**) to allow it for the current session, `Esc` (or click **Deny**) to
refuse it, or toggle **Remember** (`R` or click) before allowing to save the choice in
`~/.local/state/pila/capture-permissions.json`. Allow takes effect only half a second after the
prompt appears, so an `Enter` typed into an app as it pops up doesn't allow anything. A denied
program isn't asked again until it starts over (connects again). While the screen is locked,
programs that would be asked are refused. List grants with `pila msg capture-permissions`, and
revoke one with `pila msg forget-capture-permission <id>` (or `all`), which also stops active
captures from that program. Portal screen sharing and screenshots are unaffected and always use
Pila's picker.

The executable path is what's checked, so this guards against programs you didn't expect, not
against a program set on getting around it: any unsandboxed process running as you can also edit
the configuration or run its code inside an allowed program. Interpreters are one program for all
their scripts (allowing `python3` allows every Python script), and on Nix the store path changes
with each update (match it with `*`, such as `/nix/store/*-grim-*/bin/grim`). A program updated
while running keeps its path.

## Portals, polkit, and the keyring

Desktop apps expect three session services that full desktop environments start automatically:
desktop portals (file chooser, etc.), a polkit authentication agent (administrator password
prompts), and a keyring (saved passwords). Pila doesn't implement these itself; standard tools
provide them. On a TTY, Pila exports `WAYLAND_DISPLAY`, `DISPLAY`, `XAUTHORITY`,
`XDG_CURRENT_DESKTOP=pila`, and `[environment]` to D-Bus and `systemd --user` automatically, so you
don't need `dbus-update-activation-environment` in `autostart`.

**Portals.** `xdg-desktop-portal` selects backends via `pila-portals.conf` (matched by
`XDG_CURRENT_DESKTOP`). Pila's default `/etc/xdg/xdg-desktop-portal/pila-portals.conf` routes
`ScreenCast`, `Screenshot`, and `GlobalShortcuts` to Pila and everything else (like the file
chooser) to `xdg-desktop-portal-gtk`. If you prefer GNOME's file chooser, install
`xdg-desktop-portal-gnome` and Nautilus and create `~/.config/xdg-desktop-portal/pila-portals.conf`.
Because a user file replaces the system file completely, keep the three `pila` lines intact:

```ini
[preferred]
default=gnome;gtk
org.freedesktop.impl.portal.ScreenCast=pila
org.freedesktop.impl.portal.Screenshot=pila
org.freedesktop.impl.portal.GlobalShortcuts=pila
# These require GNOME Shell when using the GNOME backend.
org.freedesktop.impl.portal.Access=gtk
org.freedesktop.impl.portal.Notification=gtk
org.freedesktop.impl.portal.Inhibit=gtk
```

Log out and back in, or run `systemctl --user restart xdg-desktop-portal`.
- **Qt apps:** set `QT_QPA_PLATFORMTHEME = "xdgdesktopportal"` under `[environment]` to use the
  portal file chooser.
- **Firefox:** set `widget.use-xdg-desktop-portal.file-picker` to `1` in `about:config`.
- **Dark mode (GTK 4):** run `gsettings set org.gnome.desktop.interface color-scheme prefer-dark`
  (read by both GTK and GNOME portal backends).

**Polkit agent.** Apps requiring privilege elevation (`pkexec`, disk utilities, software managers)
need a running polkit agent; `pila-desktop` doesn't install one, so without one `pkexec` fails
immediately. Add an agent to `autostart` (remember that defining `autostart` replaces the default
list, so keep the default tools you want):

```toml
autostart = [
    "swaybg",
    "waybar",
    "mako",
    "swayidle -w timeout 300 'swaylock -f' timeout 600 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
    "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
]
```

The binary path varies by distro (the example above is Arch's `polkit-gnome`). `lxqt-policykit-agent`
and `mate-polkit` work too, though you should only run one agent at a time.

**Keyring.** Apps that store passwords over `libsecret` (browsers, mail clients, Git) use a Secret
Service provider such as GNOME Keyring or KeePassXC (with Secret Service enabled). To unlock GNOME
Keyring automatically at login, ensure your display manager's PAM config (e.g., `/etc/pam.d/greetd`)
includes `auth optional pam_gnome_keyring.so` and `session optional pam_gnome_keyring.so auto_start`.
It normally starts via D-Bus activation; if not, add
`gnome-keyring-daemon --start --components=secrets` to `autostart`.

## Locking the screen and idling

Any lock screen implementing `ext-session-lock` works (`swaylock`, or a desktop shell's built-in
locker). While locked, only the lock screen receives keyboard and pointer input and Pila's bindings
are disabled (`Ctrl+Alt+F1..F12` VT switching still works).

Idle timeouts are handled by `swayidle`, which the default `autostart` runs to lock the screen with
`swaylock` after 5 minutes of inactivity and before system sleep, and to power off monitors after
10 minutes. Any input wakes the monitors (consuming that first key or click). Visible video players
that request idle inhibition keep the session awake. To change timings or disable automatic
locking, customize the `swayidle` entry in `autostart` (takes effect on next login or manual
restart):

```toml
autostart = [
    "waybar",
    "mako",
    # Lock after 15 minutes, turn monitors off after 20, lock before sleep.
    "swayidle -w timeout 900 'swaylock -f' timeout 1200 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
]
```

See `man swayidle` for additional hooks (`idlehint`, logind `lock`/`unlock`). `before-sleep`
requires logind (systemd or elogind), while inactivity timeouts work anywhere. If your desktop
shell manages idle and locking itself, simply omit `swayidle` from `autostart`. You can also use
`wlopm --off '*'` with a matching `resume 'wlopm --on "*"'` hook instead of
`pila msg action power-off-monitors`. Legacy configs containing `[idle] monitors-off` still load
with a warning pointing to the equivalent `swayidle` line.

**If the lock screen crashes:** For security, the session remains locked. Switch to an unused TTY
(`Ctrl+Alt+F3`, or another free VT), log in, and launch a new locker on Pila's display, for example:
`WAYLAND_DISPLAY=wayland-1 swaylock` (check `ls $XDG_RUNTIME_DIR` for the socket name). Unlocking
it restores your session.

## Games and X11 apps

**Xwayland:** X11 apps (legacy programs, Steam, many games) run via Xwayland, started automatically
on demand when the first X11 client connects. `DISPLAY` is always exported, but no X server runs
until needed. Access is authenticated with a per-session cookie in `XAUTHORITY` (stored under
`$XDG_RUNTIME_DIR`, readable only by your user; Flatpak apps with X11 permission receive it through
Flatpak). Clipboard sharing works both ways between Wayland and X11, and games can lock or confine
the pointer.

Once launched, Xwayland normally stays running until Pila exits. To shut it down automatically when
no X11 clients remain (restarting if another opens later):

```toml
[xwayland]
idle-timeout = 10   # seconds
```

Note that background X11 daemons (settings daemons, XIM) keep Xwayland alive, and shutting Xwayland
down resets X-server state such as `xrdb`, `setxkbmap`, or `xmodmap`, so leave `idle-timeout` unset if
your startup scripts rely on those.

To disable X11 completely (no `DISPLAY`, no `XAUTHORITY`, encouraging dual-backend apps to use
Wayland):

```toml
[xwayland]
enabled = false
```

If disabled via live reload while X11 apps are already open, Xwayland stays alive until they close
(with `idle-timeout`) or Pila exits, while newly launched apps won't see `DISPLAY`. Leaving
Xwayland uninstalled has the same effect.

**Keyboard grabs and shortcuts inhibition:** Fullscreen VMs, remote desktops, and apps using
`keyboard-shortcuts-inhibit` or `XGrabKeyboard` receive Pila's keybindings and touchpad gestures
while focused (except `quit`). Clicking another window always releases focus.

**X11 launchers:** Launchers like `dmenu` (`launcher = "dmenu_run"`) receive keyboard focus when
opened (even over Wayland windows) and return focus on close, while X11 menus and tooltips don't
steal focus.

**Steam game rule example:** Disable window animations for Steam games:

```toml
[[window-rule]]
app-id = "steam_app_*"
animations = false
```

**Variable refresh rate (VRR):** Set `adaptive-sync = true` inside an `[[output]]` block on
monitors that support it.

**Direct scanout:** On a TTY (primary-GPU monitors), when a fullscreen game or video is the sole
visible surface on a monitor, Pila hands its buffer directly to the display hardware without
compositing, saving GPU work and battery. If a fullscreen app flickers or shows wrong colors, set
`direct-scanout = false` (applies immediately on save).

**Screen tearing:** Games with vsync disabled (or Vulkan immediate present mode) can present frames
immediately instead of waiting for vblank, trading lower latency for tearing. Pila allows tearing on
a TTY while the game is fullscreen and nothing else on that monitor changes (moving the pointer
over it reverts to vsync). Games with vsync enabled never tear. Set `tearing = false` to disable
tearing globally.

**Late rendering:** On a TTY, Pila delays compositing each frame until just before the upcoming
monitor refresh, rather than drawing right after the previous one, so recent app frames and pointer
motion make it onto the screen one refresh sooner. The safety margin adapts dynamically to recent
CPU and GPU frame times; if a frame misses anyway, Pila temporarily falls back to immediate drawing
on that monitor for two seconds. Late rendering pauses automatically during animations, briefly
after windows or layers open or resize, and during tearing or adaptive sync. Set
`render-late = false` to disable it.

## Window and layer rules

Rules customize individual windows or layer surfaces. A `[[window-rule]]` matches by `app-id`,
`title`, or `tag` (supporting `*` wildcards) and can override: `animations`, `open-on-workspace`,
`open-on-output`, `open-focused`, `size`, `position`, `open-maximized`, `open-fullscreen`,
`always-on-top`, `all-workspaces`, `title-bar`, `corner-radius`, `opacity`, `blur`, `popup-blur`,
`popup-corner-radius`, and `shadow`. Rules re-evaluate whenever a window's title changes (such as a
browser switching tabs), except for open-time properties (`open-*`, `size`, `position`,
`always-on-top`, `all-workspaces`). A `[[layer-rule]]` matches bars, launchers, and notifications
by `namespace` and controls `animations` and `blur`.

```toml
[[window-rule]]
app-id = "kitty"
opacity = 0.95
blur = true

[[window-rule]]
app-id = "thunderbird"
open-on-workspace = 3
```

By default, new windows open on the monitor under the pointer. `open-on-output` targets a specific
monitor by connector (`eDP-1`) or make/model (`"Make Model"`, same as `[[output]]`), falling back
to the pointer's monitor if unavailable. `open-focused = false` opens a window without stealing
keyboard focus. Combining both is useful for keeping background or development windows (such as a
nested Pila instance, whose app ID is `pila`) out of your way:

```toml
[[window-rule]]
app-id = "pila"
open-on-output = "eDP-1"
open-focused = false
```

Run `pila msg windows` to inspect every window's `app_id` and `title`. See `config.example.toml`
for all rule fields.

## Scripting with `pila msg`

`pila msg` queries a running Pila instance and executes any bindable action:

```bash
pila msg windows                 # list all windows with ID, app ID, title, and workspace
pila msg --json outputs | jq     # inspect monitors as JSON
pila msg action workspace 2      # run any [bindings] action
pila msg focus <id>              # focus a window by ID
pila msg close <id>              # close a window by ID
pila msg minimize <id>           # minimize a window (focus restores it)
pila msg switch                  # inspect active Mod+Tab switcher state
pila msg screen-shares           # list the screen shares remembered for apps
pila msg forget-screen-share <id> # revoke one (or all)
pila msg capture-permissions     # list direct screen capture permissions
pila msg forget-capture-permission <id> # revoke one (or all)
pila msg --json subscribe        # stream live window, workspace, and output changes
```

Always pass `--json` in scripts; plain-text formatting is for human reading and may change. Bars and
daemons should use `subscribe` instead of polling. Full socket and schema details are in
[IPC.md](https://github.com/igalmarino/pila/blob/main/docs/IPC.md).

## Troubleshooting

**Logs:** Session logs are written to `~/.local/state/pila/pila.log` (or
`$XDG_STATE_HOME/pila/pila.log`), with the previous run in `pila.log.old`. Under systemd, logs also
go to the journal (`journalctl -t pila -b`). For verbose logging, start with `RUST_LOG=debug`
(e.g., `RUST_LOG=debug pila-session` on a TTY, or `RUST_LOG=debug scripts/tty` from source).

**`pila-report`:** Generates `pila-report.txt` containing the Pila version, OS/GPU/monitor info,
config validation result, and the tail of the log (redacting your username, hostname, and home
directory). Nothing is uploaded automatically; inspect the file before attaching it to a bug report.

**`pila-check`:** Runs a ~1-minute hardware smoke and frame-time benchmark on a spare TTY
(`Ctrl+Alt+F3`) without disturbing your running session. It launches Pila with your monitors and
input config, runs real programs through windows, workspaces, captures, blur, and fullscreen, exits
automatically, and prints `PASS`/`FAIL` along with CPU/GPU frame times and display latency. Logs go
to `~/.local/state/pila/pila-tty.log`. Run `pila-check --nested` to test inside a window,
`PILA_CHECK_RENDER_LATE=false pila-check` to compare with late rendering disabled, or
`pila-check --help` for all options.

| Problem | What to check |
|---|---|
| Pila session won't start | Log into another desktop session and check `~/.local/state/pila/pila.log`. Run `pila --check-config` to rule out config errors. |
| Notification says the config is invalid | Fix the reported line and save again, or run `pila --check-config`. Remember that keybindings must be under `[bindings]`. |
| Config edits don't take effect | The saved file has an error, so Pila kept the last valid config. Run `pila --check-config` to see the exact line and error (notifications require a running notification daemon). |
| Holding volume or brightness keys only steps once | Use table syntax with `repeat = true`: `"XF86AudioRaiseVolume" = { action = "spawn wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+", repeat = true }`. |
| Volume, brightness, or media keys do nothing | Defaults use `wpctl` (`wireplumber`), `brightnessctl`, and `playerctl`. Install them or rebind the keys (e.g., `pactl` on PulseAudio). Media keys require an MPRIS-compatible player. Pila shows a one-time notification when a bound program is missing (default `autostart` tools are skipped quietly). |
| `Mod+Shift+S` or `Mod+Ctrl+S` does nothing | Install `grim` (plus `slurp` and `wl-clipboard` for region capture). Pila notifies you if one is missing. |
| Clicking links in the terminal does nothing | Install `xdg-utils` (provides `xdg-open`). |
| No notifications from Pila | Make sure a notification daemon (such as `mako`) is running on the session bus; Pila logs once if none is found. |
| Administrator password prompts fail (`pkexec` exits immediately) | Start a polkit authentication agent from `autostart` (see [Portals, polkit, and the keyring](#portals-polkit-and-the-keyring)). |
| Screen sharing or app screenshots fail, or no picker appears | Install `xdg-desktop-portal` and `pipewire`, and launch via your login manager or `pila-session` (not nested). If you have a custom `~/.config/xdg-desktop-portal/pila-portals.conf`, ensure it sets `ScreenCast=pila` and `Screenshot=pila` (not `wlr`). |
| Desktop shell fails to turn off monitors on idle | Some shells run `wlr-randr --off` without an output name. Use `pila msg action power-off-monitors` instead. |
| X11 apps won't start | Install `Xwayland`; check the log for startup errors. |
| Title bars show no text | Install at least one TrueType or OpenType font. |
| Stuck on a locked screen after locker crashed | See [Locking the screen and idling](#locking-the-screen-and-idling). |
| `Mod+D` or tapping `Mod` does nothing | `launcher` is empty (`""`) or the configured launcher (`fuzzel` by default) isn't installed. |
| No bar or terminal at login | Pila only launches programs in `autostart` and never auto-opens a terminal (`Mod+Return` opens one). Install `waybar` or add your preferred bar/terminal to `autostart`. |
| App needs `Mod` + mouse wheel | Unbind it with `"Mod+WheelScrollDown" = "none"` and `"Mod+WheelScrollUp" = "none"` under `[bindings]` (in nested mode, `Mod` is `Alt`). |
| Tapping `Mod` opens the launcher unexpectedly | Disable it with `"Mod" = "none"` under `[bindings]` (in nested mode, `Mod` is `Alt`). |
| Fullscreen window flickers or has wrong colors | Set `direct-scanout = false` (applies on save) and report the issue with `pila-report.txt`. |
| Screen froze | Switch to a free TTY (`Ctrl+Alt+F3`), log in, run `pkill pila`, and report the bug with the log. |
| Pila crashed back to the login screen | Report the bug with `pila.log.old` or `pila-report.txt` (look for the `"pila crashed"` line and backtrace). |

Report bugs on [GitHub Issues](https://github.com/igalmarino/pila/issues) (attaching
`pila-report.txt` or logs). Questions and feature ideas belong in
[Discussions](https://github.com/igalmarino/pila/discussions).
