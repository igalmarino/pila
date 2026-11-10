# Pila user manual

Pila is a stacking Wayland compositor: windows overlap and you move them around, as on most
desktops. Pila draws the windows, their title bars and shadows, and handles the keyboard, mouse,
touchpad and monitors. The rest of a desktop (a bar, a launcher, notifications, a wallpaper, a
lock screen) comes from separate programs, or from a shell such as Noctalia that has them all.

This manual says how to use Pila and set it up. Every configuration option is described in
[`config.example.toml`](../config.example.toml) (installed by Pila's packages, usually in
`/usr/share/doc/pila`, and in its source); `man pila` has the command line. Installing Pila is in the
[README](../README.md#running-pila).

- [Starting and leaving Pila](#starting-and-leaving-pila)
- [The Mod key and the default bindings](#the-mod-key-and-the-default-bindings)
- [Windows](#windows)
- [Workspaces](#workspaces)
- [Monitors](#monitors)
- [Keyboard, mouse and touchpad](#keyboard-mouse-and-touchpad)
- [The configuration file](#the-configuration-file)
- [Building your desktop](#building-your-desktop)
- [Screenshots and screen sharing](#screenshots-and-screen-sharing)
- [Locking the screen and idling](#locking-the-screen-and-idling)
- [Games and X11 apps](#games-and-x11-apps)
- [Window and layer rules](#window-and-layer-rules)
- [Scripting with `pila msg`](#scripting-with-pila-msg)
- [Troubleshooting](#troubleshooting)

## Starting and leaving Pila

- **From the login screen:** pick *Pila* in the session menu (on GDM, the gear icon on the
  password screen). This is the usual way.
- **From a TTY:** log in on a text console and run `pila-session`, which starts Pila the way the
  login screen does, with its log in `~/.local/state/pila/pila.log`.
- **Nested:** run `pila` from another desktop and it opens as a window there. Nothing else on
  your system changes, so it's a safe way to try a config. Nested, the Mod key is Alt (the
  desktop around it keeps Super), and the mouse, touchpad and monitors belong to that desktop.

At startup Pila runs the programs in `autostart`, or a terminal if there are none.

To leave, press **Mod+Shift+E**: Pila quits and you're back at the login screen or the TTY. If the
screen stops responding, **Ctrl+Alt+F1** to **F12** switch to another virtual terminal, where you
can log in and run `pkill pila`.

## The Mod key and the default bindings

Most bindings use the Mod key: **Super** (the Windows key) from the login screen or a TTY, and
**Alt** when nested. `mod` in the configuration sets it to `super`, `alt` or `ctrl`.

| Keys | What they do |
|---|---|
| `Mod+Return` | Open a terminal (`terminal`, or the first one installed of kitty, foot, alacritty and others) |
| `Mod+D` | Open the launcher (only if `launcher` is set) |
| `Mod` | Tapped alone (pressed and released with nothing else in between): open the launcher too |
| `Mod+Tab`, `Mod+J` | Switch to the previous window; keep Mod held and press again for older ones |
| `Mod+Shift+Tab` | The same, the other way |
| `Alt+Tab`, `Alt+Shift+Tab` | The same as `Mod+Tab` and `Mod+Shift+Tab`, whatever Mod is |
| `Mod+M`, `Mod+Up` | Maximize; again, restore |
| `Mod+N` | Minimize |
| `Mod+F` | Fullscreen, or leave it |
| `Mod+Left`, `Mod+Right` | Fill the left or right half of the screen; again, restore |
| `Mod+Down` | Restore a snapped or maximized window |
| `Mod+Shift+Left`, `Mod+Shift+Right`, `Mod+Shift+Up`, `Mod+Shift+Down` | Move the window 40 pixels |
| `Mod+Ctrl+Right`, `Mod+Ctrl+Down` | Make the window wider, taller |
| `Mod+Ctrl+Left`, `Mod+Ctrl+Up` | Make it narrower, shorter |
| `Mod+C` | Center the window |
| `Mod+T` | Keep the window above the others; again, stop |
| `Mod+P` | Keep the window on all workspaces; again, stop |
| `Mod+H` | Minimize the workspace's windows (show the desktop); again, bring them back |
| `Mod+O` | Go to the next monitor (the pointer and the focus) |
| `Mod+Shift+O` | Send the window to the next monitor |
| `Mod+1` to `Mod+9` | Go to that workspace |
| `Mod+Shift+1` to `Mod+Shift+9` | Send the window to that workspace |
| `Mod+Page_Down`, `Mod+Page_Up` | Next, previous workspace |
| `Mod+Shift+Page_Down`, `Mod+Shift+Page_Up` | Send the window to the next, previous workspace |
| `Mod+Shift+S` | Screenshot of a region you drag, to the clipboard (needs grim, slurp and wl-clipboard) |
| `Mod+Ctrl+S` | Screenshot of the monitor under the pointer, to `~/Pictures` (needs grim) |
| `Mod+Shift+Q` | Close the window |
| `Mod+Shift+E` | Quit Pila |
| `Mod` + left drag | Move a window from anywhere in it |
| `Mod` + right drag | Resize a window from its nearest corner |
| `Ctrl+Alt+F1` to `F12` | Switch to another virtual terminal (not nested) |

Any of these but the two drags and the VT switch can be changed or removed (the drags follow
the `mod` setting), and any key bound to an action or a command, in `[bindings]`: see
[The configuration file](#the-configuration-file). The touchpad gestures are
in [Keyboard, mouse and touchpad](#keyboard-mouse-and-touchpad).

## Windows

**With the mouse.** Windows that let Pila decorate them get a title bar with minimize, maximize
and close buttons: drag the bar to move the window, and drag any edge or corner to resize it.
Apps that draw their own title bar (GTK apps, browsers) work the way they always do. Mod plus
the left button moves any window from anywhere in it, and Mod plus the right button resizes it.
Right-clicking a title bar, Pila's or an app's own, opens the window menu: minimize, maximize,
always on top, on all workspaces, close. Up, Down and Enter work in it too; Escape or a click
elsewhere closes it.

**Snapping.** Drag a window with the pointer at the left or right edge of a monitor to make it
fill that half, near a corner to fill that quarter, or at the top edge to maximize it. A preview
shows where it would go before you let go. `Mod+Left` and `Mod+Right` do the halves from the
keyboard; the quarters are the actions `snap-top-left`, `snap-top-right`, `snap-bottom-left`
and `snap-bottom-right`, with no default binding. A snapped window remembers where it was:
snapping the same side again, `Mod+Down`, or dragging it away brings it back.

**Maximized and fullscreen.** `Mod+M` and `Mod+Up` maximize (the window fills the monitor, minus
any bar), `Mod+F` makes it fullscreen; pressing them again restores the window. Maximized and fullscreen windows have square corners and
no shadow.

**From the keyboard.** `Mod+Shift` with an arrow moves the focused window, `Mod+Ctrl` with an
arrow resizes it (right and down grow it, left and up shrink it), and `Mod+C` centers it.

**Switching windows.** `Mod+Tab` goes to the window you used before, so a quick `Mod+Tab`
toggles between the last two. Keep Mod held and press Tab again to go further back; each window
comes forward as you reach it, and releasing Mod settles on it. `Mod+Shift+Tab` goes the other
way. `Alt+Tab` and `Alt+Shift+Tab` do the same whatever Mod is, as on other desktops, so apps
no longer get them; `"Alt+Tab" = "none"` and `"Alt+Shift+Tab" = "none"` in `[bindings]` give
them back. Clicking a
window, or a taskbar entry in your bar, focuses it too.

**Windows asking for attention.** An app can ask for your attention without taking the focus
(a chat message, a finished download): its title bar turns amber (`[colors] bar-urgent`), and
bars that show workspaces mark its workspace. Focusing the window clears it.

**Minimizing.** `Mod+N` or the title bar's button hides a window. It comes back through `Mod+Tab`
(it shows while it's selected) or by clicking it in a taskbar. `Mod+H` minimizes all the windows
of the workspace at once, and again brings them back.

**Above the others and on all workspaces.** `Mod+T` keeps a window above the others, even over a
fullscreen one; `Mod+P` keeps it on every workspace. Both work until you press them again, and
window rules can set them when a window opens.

**Closing.** `Mod+Shift+Q` or the close button asks the window to close; the app may ask you
first, as it would for unsaved work.

**Windows coming back where they were.** Apps that ask for it (xdg-session-management) get
their windows' place, size and maximized or fullscreen state back when they open them again,
even after Pila restarts. Pila keeps that in `~/.local/state/pila/sessions.json`; a nested Pila,
or a second one started next to your session, keeps it in memory only.

## Workspaces

There are four workspaces unless `workspaces` says otherwise (1 to 32). A workspace spans all
monitors: switching to another one changes what every monitor shows. Windows on all workspaces
(`Mod+P`) stay where they are.

- `Mod+1` to `Mod+9` go to a workspace, and `Mod+Shift+1` to `Mod+Shift+9` send the focused
  window there. Only as many digits as there are workspaces are taken; the others still reach
  the apps.
- `Mod+Page_Down` and `Mod+Page_Up` go to the next and previous workspace; they stop at the last
  and the first.
- On a touchpad, four fingers left go to the next workspace and four fingers right to the
  previous one.
- Bars and pagers that support ext-workspace, such as Noctalia's, show them and switch between
  them.

A window rule's `open-on-workspace` opens an app's windows on a given workspace.

## Monitors

Pila uses every monitor that's plugged in, and takes monitors plugged in or out while it runs.
Without settings, a monitor uses its preferred mode at scale 1, to the right of the others.
If that mode doesn't come on (two 4K monitors on one dock can be more than its link carries),
Pila tries the same resolution at lower refresh rates, then smaller ones, and says in its log
which mode it used.

`[[output]]` sections in the configuration set a monitor's mode (resolution and refresh rate),
scale (fractional scales such as 1.25 work), position, rotation and variable refresh rate, or
leave it off. A section names a monitor by its connector (`eDP-1`, `DP-1`, `HDMI-A-1`) or by
its make and model, optionally with its serial number, so the settings follow the monitor to
whichever port it's plugged into. `wlr-randr` lists the connected monitors with their names,
makes and models; Pila's log has them too.

Programs that change monitor settings at runtime (`wlr-randr`, a shell's display settings) work
too, until you change that monitor's section in the configuration.

**Monitors on another GPU.** On a laptop with two GPUs, the HDMI or DisplayPort output is often
wired to the dedicated one, and a dock may have a GPU of its own. Such monitors stay off unless
the configuration says `other-gpus = true`: Pila then draws everything with the seat's main GPU
and copies the picture of such a monitor to the GPU it's on, which shows it. It's off by default
because it's new and hasn't been tried on such a machine yet: a report of how it goes is very
welcome. Saving the file turns it on or off at once, and `pila-report` says which GPUs Pila uses
and which monitors are on another one.

`Mod+O` moves the pointer and the focus to the next monitor, and `Mod+Shift+O` sends the focused
window there, keeping its place, or still filling the monitor or the same half.

To turn the monitors off without unplugging them, for instance when idle, run
`pila msg action power-off-monitors` (or bind the action to a key): any key, click or move of the
pointer turns them back on, and that first key or click does nothing else. Through `pila msg` it
works with the screen locked too, as idling usually locks first; a key binding doesn't run while
the screen is locked. `pila msg action power-on-monitors` turns them
on from a script. `wlopm --off '*'` and `wlopm --on '*'` work as well; monitors a tool turned off
stay off until that tool turns them on. `pila msg outputs` marks the monitors that are off
`[off]`.

## Keyboard, mouse and touchpad

**Keyboard layout.** `[keyboard]` sets the layout, variant, model and options, with the names
xkb uses (`localectl list-x11-keymap-layouts` lists them where systemd is installed). Several
layouts, with a key combination to switch between them:

```toml
[keyboard]
layout = "us,es"
options = "grp:alt_shift_toggle"
```

A binding can switch too, with the action `switch-layout next`, `prev` or a number from 1 (in
the order of `layout`), and so can a bar or a script: `pila msg action switch-layout next`.
`pila msg keyboard` lists the layouts and says which is in use, and `pila msg subscribe keyboard`
follows it as it changes.

`repeat-delay` and `repeat-rate` set how soon and how fast a held key repeats.
`pila --check-config` tells you if xkb doesn't know a layout you wrote.

**Mouse and touchpad.** `[input.touchpad]` and `[input.mouse]` set natural scrolling, pointer
speed and acceleration, tap to click (on by default), disabling the touchpad while typing, how
a clickpad tells left from right clicks, and middle-click emulation. Settings you leave out
keep libinput's defaults for the device.

**Touchpad gestures.** Pila handles these swipes itself:

| Swipe | What it does |
|---|---|
| `swipe-3-left`, `swipe-3-right` | Switch windows, like `Mod+Tab` and `Mod+Shift+Tab`; keep swiping for older ones |
| `swipe-3-up` | Maximize, or restore |
| `swipe-3-down` | Restore a snapped or maximized window |
| `swipe-4-left`, `swipe-4-right` | Next, previous workspace |

`[gestures]` changes them, with any action a key binding can have, for 3, 4 or 5 fingers. Other
gestures (pinching, finger counts Pila doesn't use) reach the apps.

The mouse, touchpad and gesture settings apply from the login screen or a TTY; nested, the
desktop around Pila handles those devices. The keyboard settings apply either way.

## The configuration file

Pila reads `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila/config.toml`). Without one,
it uses its defaults. To start one, copy the example, which packages install in
`/usr/share/doc/pila` (it's also in Pila's source):

```bash
mkdir -p ~/.config/pila
cp /usr/share/doc/pila/config.example.toml ~/.config/pila/config.toml
```

Every line in the example is commented out, so the copy changes nothing until you edit it. Each
value says whether it's the default or an example; take the `#` off a line to use it.

**Changes apply when you save**, all but `autostart`, which only runs when Pila starts. If the
saved file has a mistake, Pila keeps the configuration it was running with and shows a
notification with the line and what's wrong (and writes it to the log). If the file is invalid
when Pila starts, Pila starts with the defaults and says why.

`pila --check-config` checks the file without starting Pila, the keyboard layout included, and
prints what's wrong.

**Key bindings** go in `[bindings]`: a key combination, and an action or a command.

```toml
[bindings]
"Mod+Shift+Return" = "spawn foot -e htop"
"XF86AudioRaiseVolume" = "spawn wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+"
"Mod+J" = "none"          # remove a default binding
```

Keys are xkb key names (`Return`, `Tab`, `q`, `F5`, `XF86AudioMute`); the modifiers (`Mod`,
`Shift`, `Ctrl`, `Alt`, `Super`) must match exactly. `spawn` runs the rest of the line with
`sh`, with the name of the monitor under the pointer in `PILA_OUTPUT`. The actions are listed
in `config.example.toml`.

The environment variables `PILA_MOD`, `PILA_TERMINAL` and `PILA_LAUNCHER` override `mod`,
`terminal` and `launcher`.

## Building your desktop

Pila starts the programs listed in `autostart` once it's ready (X11 support included). Everything
they open, bars, launchers, notifications and wallpapers, is placed by Pila the way those
programs ask.

**With Noctalia**, one program gives you the bar, launcher, notifications, wallpaper and lock
screen:

```toml
autostart = ["noctalia"]
launcher = "noctalia msg panel-toggle launcher"    # Mod+D opens Noctalia's launcher

# Noctalia animates its own panels; without this, Pila animates them as well.
[[layer-rule]]
namespace = "noctalia-*"
animations = false
```

Noctalia opens its panels on the monitor of the window you're using. Its own monitor off
(`screen_off`, `dpms-off`) doesn't work on Pila: Noctalia 5.4 runs `wlr-randr --off` without
naming a monitor. Give its idle behavior Pila's action instead (Settings → Idle, or its
`[idle.behavior.screen-off]`), with no resume command: any input turns the monitors back on.

```toml
# In Noctalia's settings file, not Pila's
[idle.behavior.screen-off]
timeout = 660
action = "command"
command = "pila msg action power-off-monitors"
```

Its Log out ends the session through logind (`loginctl terminate-session`), which works where
systemd or elogind runs it. Elsewhere, or to quit Pila directly, set that button's command to
`pila msg action quit` (Settings → Power → Session panel).

**With Dux Shell** (DankMaterialShell):

```toml
autostart = ["dms run"]
launcher = "dms ipc call spotlight toggle"         # Mod+D opens its launcher

# Dux Shell animates its own panels; without this, Pila animates them as well.
[[layer-rule]]
namespace = "dms:*"
animations = false

# Its settings and welcome windows draw their own title bar.
[[window-rule]]
app-id = "dev.duxlabs.shell"
title-bar = false
```

Dux Shell turns monitors off on idle through the standard protocol, which works as it is. For
Log out, give its power settings a custom command (`customPowerActionLogout`):
`pila msg action quit`.

Both shells work on Pila through standard protocols: bar, workspaces, taskbar and dock, launcher,
notifications, clipboard history, screenshots, night light, display settings and power, idle and
the lock screen. Neither knows Pila by name yet, so the features they build on a compositor's own
commands are missing: Dux Shell's overview, window rules and keybinding editor, and switching the
keyboard layout from either shell's bar (Pila's keyboard settings and `Mod` bindings still work;
a binding to `switch-layout next` switches layouts).
`dms doctor` says "No supported Wayland compositor found"; that line is all it means.

**With separate tools**, for example:

```toml
launcher = "fuzzel"
autostart = [
    "swaybg -i ~/Pictures/wallpaper.png",
    "waybar",
    "mako",
    "swayidle -w timeout 300 'swaylock -f' before-sleep 'swaylock -f'",
]
```

Clipboard managers that use ext-data-control, night light (`wlsunset`, through gamma control) and
screen recorders that use screencopy (`wf-recorder`) work too.

Bars and taskbars can list, focus, minimize and close windows (waybar's taskbar module, for
instance) and switch workspaces. What no protocol tells, a bar gets from `pila msg`; a waybar
module that shows the keyboard layout as it changes and switches it on a click:

```json
"custom/layout": {
    "exec": "pila msg --json subscribe keyboard | jq --unbuffered -r '.layouts[]? | select(.active) | .name'",
    "on-click": "pila msg action switch-layout next"
}
```

**Programs started by Pila** get `XDG_CURRENT_DESKTOP=pila` and `PILA_SOCKET`, for `pila msg`.
Programs such as kitty open links with `xdg-open`, from the `xdg-utils` package: without it, a
click on a link does nothing.

**Notifications from Pila** (an invalid configuration, for instance) are sent with
`notify-send`, from the `libnotify` package, and shown by your notification daemon (Noctalia,
mako, ...). Without either, they only reach the log.

## Screenshots and screen sharing

`Mod+Shift+S` lets you drag a region and puts it on the clipboard; `Mod+Ctrl+S` saves the
monitor under the pointer to `~/Pictures`. They need `grim`, and the first also `slurp` and
`wl-clipboard`. `grim` and other screenshot tools work on their own too, and so do a shell's
own screenshot features.

**Screen sharing** in browsers, OBS and video calls goes through the desktop portal:
`xdg-desktop-portal-wlr` for the screen, and `xdg-desktop-portal-gtk` for everything else (the
file chooser, for instance). Pila installs their configuration for a Pila session. When an app
asks to share, the pointer becomes a crosshair: click the monitor to share (this uses `slurp`).

**Sharing a single window** needs a menu to pick it from, since `slurp` can only click
monitors. With `fuzzel` installed (`wofi`, `rofi`, `bemenu` or `wmenu` work too), copy
`/etc/xdg/xdg-desktop-portal-wlr/pila` to `~/.config/xdg-desktop-portal-wlr/pila` if it isn't
there yet, change its two `chooser_` lines to:

```ini
chooser_type=dmenu
chooser_cmd=fuzzel -d -l 10 -p 'Select a source to share: '
```

and restart the portal (`systemctl --user restart xdg-desktop-portal-wlr`, or log out and back
in). The menu then lists monitors and windows. A shared window keeps updating while it's
covered, minimized or on another workspace.

Screen sharing needs the session's D-Bus bus, which the login screen and `pila-session` set up;
a nested Pila doesn't have it.

## Locking the screen and idling

Lock screens that use ext-session-lock work: `swaylock`, or your shell's own (Noctalia has one).
While the screen is locked, only the lock screen gets the keyboard and the mouse, and Pila's
bindings don't run.

To lock and turn the screens off after a while without input, use `swayidle` (or your shell's
idle settings):

```toml
autostart = [
    "swayidle -w timeout 300 'swaylock -f' timeout 600 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
]
```

A video playing in a visible window keeps the session from idling, if the player asks for it,
as most do.

If the lock screen crashes, the session stays locked, for safety. Switch to a free virtual
terminal (`Ctrl+Alt+F3`, or another free one: F1 and F2 may be the login screen or this very
session), log in and start a new lock screen on Pila's display, for instance
`WAYLAND_DISPLAY=wayland-1 swaylock` (`ls $XDG_RUNTIME_DIR` shows the name); unlocking it gives
you the session back.

## Games and X11 apps

X11 apps (older programs, many games, Steam) run through Xwayland, which Pila starts by itself;
the clipboard works between X11 and Wayland apps. Games can lock and confine the pointer.

Apps that ask for every key while they have the focus (virtual machines, remote desktops:
keyboard-shortcuts-inhibit) get Pila's bindings and touchpad gestures too, all but the one that
quits Pila.

A rule turns off the window animations for Steam games:

```toml
[[window-rule]]
app-id = "steam_app_*"
animations = false
```

On monitors that support it, `adaptive-sync = true` in an `[[output]]` section turns on variable
refresh rate.

## Window and layer rules

Rules change how particular windows or panels behave. A `[[window-rule]]` matches windows by
app id, title or tag, with `*` for any text, and can set: animations, the workspace to open on,
size, position, opening maximized or fullscreen, staying above the others or on all
workspaces, the title bar, corner radius, opacity, blur and shadow. A window's rules follow its
title as it changes (a browser's page title), except what applies when it opens. A
`[[layer-rule]]` matches bars, launchers and notifications by namespace, and sets their
animations and blur.

```toml
[[window-rule]]
app-id = "kitty"
opacity = 0.95
blur = true

[[window-rule]]
app-id = "thunderbird"
open-on-workspace = 3
```

`pila msg windows` shows each window's app id and title. Every option is in
`config.example.toml`.

## Scripting with `pila msg`

`pila msg` asks a running Pila about its windows, monitors and workspaces, and runs any action a
binding can run:

```bash
pila msg windows                 # every window, with its id, app id, title and workspace
pila msg --json outputs | jq     # the monitors, as JSON for scripts
pila msg action workspace 2      # any action, as in [bindings]
pila msg focus <id>              # focus a window
pila msg close <id>              # close a window
pila msg --json subscribe        # windows, workspaces and monitors as they change
```

Scripts should use `--json`: the plain text is for reading and may change. A bar or script that
follows the windows or workspaces subscribes instead of asking over and over. The full
description, the socket and the events included, is in [IPC.md](IPC.md).

## Troubleshooting

**The log** is in `~/.local/state/pila/pila.log` (`$XDG_STATE_HOME/pila` if that's set; the run
before is in `pila.log.old`), and with
systemd also in the journal: `journalctl -t pila -b`. `RUST_LOG=debug` makes it detailed (run
`RUST_LOG=debug pila-session` from a TTY, or `RUST_LOG=debug scripts/tty` from Pila's source).

**`pila-report`** gathers what a bug report needs (version, system, GPU, monitors, whether the
configuration is valid, the end of the log) into `pila-report.txt`, without your user name,
host name and home directory. It sends nothing anywhere; read it before posting it.

| Problem | What to do |
|---|---|
| The Pila session doesn't start | Pick another session at the login screen and read the log. `pila --check-config` rules out the configuration. |
| A notification says the configuration is invalid | It gives the line and the problem; fix it and save. `pila --check-config` checks it again. A key binding must be under `[bindings]`. |
| `Mod+Shift+S` does nothing | Install `grim`, `slurp` and `wl-clipboard`; `pila-report` says if `wl-copy` is missing. |
| Clicking a link in the terminal does nothing | Install `xdg-utils`. |
| No notifications from Pila | Install `libnotify` and run a notification daemon (Noctalia, mako). |
| Screen sharing shows nothing | Install `xdg-desktop-portal-wlr` and `slurp`, and start Pila from the login screen or `pila-session`. |
| The shell doesn't turn the screens off | Some shells run `wlr-randr --off` without a monitor name, which fails; use `pila msg action power-off-monitors` (any input turns them back on). |
| X11 apps don't start | Install Xwayland; the log says if it couldn't start. |
| Title bars have no text | Install a font (any TrueType or OpenType font). |
| Stuck on a locked screen after the locker crashed | See [Locking the screen and idling](#locking-the-screen-and-idling). |
| `Mod+D` or tapping Mod does nothing | Set `launcher` to your launcher's command. |
| Tapping Mod opens the launcher and you don't want it | `"Mod" = "none"` under `[bindings]` (nested, Mod is Alt, so tapping Alt opens it). |
| The screen froze | `Ctrl+Alt+F3` (any free virtual terminal), log in, `pkill pila`; then report it with the log. |
| Pila crashed (back at the login screen) | Report it with `pila.log.old` (or `pila-report`'s file): its "pila crashed" line and the backtrace after it say where. |

Bugs go to the [issues](https://github.com/igalmarino/pila/issues), with `pila-report`'s file
or the log; questions and ideas to the
[discussions](https://github.com/igalmarino/pila/discussions).
