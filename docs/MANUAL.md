<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/igalmarino/pila/main/docs/logo/pila-logo-dark.svg">
    <img src="https://raw.githubusercontent.com/igalmarino/pila/main/docs/logo/pila-logo.svg" alt="Pila" width="420">
  </picture>
</p>

# Pila user manual

Pila is a stacking Wayland compositor: windows overlap and you move them around, as on most
desktops. Pila draws the windows, their title bars and shadows and the window switcher, and
handles the keyboard, mouse, touchpad and monitors. The rest of a desktop (a bar, a launcher,
notifications, a wallpaper, a lock screen) comes from separate, standard programs: by default waybar,
fuzzel, mako and swayidle with swaylock, where they're installed (the package `pila-desktop`
brings them all; `pila` alone is the compositor alone). A desktop shell that has them all works
too.

This manual says how to use Pila and set it up. Every configuration option is described in
[`config.example.toml`](https://github.com/igalmarino/pila/blob/main/config.example.toml) (installed by Pila's packages, usually in
`/usr/share/doc/pila`, and in its source); `man pila` has the command line. Installing Pila is in
[INSTALL.md](https://github.com/igalmarino/pila/blob/main/docs/INSTALL.md).

- [Starting and leaving Pila](#starting-and-leaving-pila)
- [The Mod key and the default bindings](#the-mod-key-and-the-default-bindings)
- [Windows](#windows)
- [Looks](#looks)
- [Workspaces](#workspaces)
- [Monitors](#monitors)
- [Keyboard, mouse and touchpad](#keyboard-mouse-and-touchpad)
- [Input methods (Chinese, Japanese, Korean, ...)](#input-methods-chinese-japanese-korean-)
- [The configuration file](#the-configuration-file)
- [Building your desktop](#building-your-desktop)
- [Screenshots and screen sharing](#screenshots-and-screen-sharing)
- [Portals, polkit and the keyring](#portals-polkit-and-the-keyring)
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

At startup Pila runs the programs in `autostart`, by default the bar `waybar`; no terminal
opens by itself: Mod+Return opens one. The first time, when it writes your configuration file
(below), Pila welcomes you with the few keys to get going (a terminal, the launcher, switching and
closing windows, quitting), as they're bound; any key or click hides it (a binding still runs:
Mod+Return hides it and opens the terminal). **Mod+Shift+/** (Mod+?) shows all the shortcuts at
any time, in sections (windows, workspaces, the mouse, Pila), with your own keys.

**With systemd**, `pila-session` runs Pila as the user service `pila.service`, bound to
`graphical-session.target` as GNOME's and KDE's sessions are: once Pila is ready, the
user services of the graphical session start (those enabled with
`WantedBy=graphical-session.target`), and so do the XDG autostart entries in
`~/.config/autostart` and `/etc/xdg/autostart` (skipping those whose `OnlyShowIn` leaves Pila
out); they stop when Pila quits. A program both there and in `autostart` starts twice. Pila's
log is then also `journalctl --user -u pila`, and a second login while a Pila session runs is
refused. `PILA_NO_SYSTEMD=1` makes `pila-session` start Pila itself instead, as it does without
systemd, where there's no such target.

To leave, press **Mod+Shift+E**: Pila quits and you're back at the login screen or the TTY. If the
screen stops responding, **Ctrl+Alt+F1** to **F12** switch to another virtual terminal, where you
can log in and run `pkill pila`.

## The Mod key and the default bindings

Most bindings use the Mod key: **Super** (the Windows key) from the login screen or a TTY, and
**Alt** when nested. `mod` in the configuration sets it to `super`, `alt` or `ctrl`.

| Keys | What they do |
|---|---|
| `Mod+Return` | Open a terminal (`terminal`, or the first one installed of kitty, foot, alacritty and others) |
| `Mod+D` | Open the launcher (`launcher`, by default `fuzzel`) |
| `Mod` | Tapped alone (pressed and released with nothing else in between): open the launcher too |
| `Mod+Tab`, `Mod+J` | Switch to the previous window; keep Mod held and press again for older ones |
| `Mod+Shift+Tab` | The same, the other way |
| `Alt+Tab`, `Alt+Shift+Tab` | The same as `Mod+Tab` and `Mod+Shift+Tab`, whatever Mod is |
| `Mod+W` | The overview: the workspace's windows spread out; click one, or pick it with the arrows and Enter; Escape closes it |
| `Mod+M` | Maximize; again, restore |
| `Mod+N` | Minimize |
| `Mod+F` | Fullscreen, or leave it |
| `Mod+Left`, `Mod+Right` | Fill the left or right half of the screen (of a quarter: the other quarter beside it); toward the side the window is already against, restore |
| `Mod+Up` | Maximize; from a half, its top quarter; from a bottom quarter, the half; from a top quarter, maximize |
| `Mod+Down` | Restore a maximized window; from a half, its bottom quarter; from a top quarter, the half; from a bottom one, restore |
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
| `Mod+WheelScrollDown`, `Mod+WheelScrollUp` | Mod and the mouse wheel: next, previous workspace, a notch each (a quick turn is one) |
| `Mod+Shift+WheelScrollDown`, `Mod+Shift+WheelScrollUp` | Send the window to the next, previous workspace |
| `Mod+Shift+S` | Screenshot of a region you drag, to the clipboard (needs grim, slurp and wl-clipboard) |
| `Mod+Ctrl+S` | Screenshot of the monitor under the pointer, to `~/Pictures` (needs grim) |
| `XF86AudioRaiseVolume`, `XF86AudioLowerVolume` | Volume up, down by 5% (up to 100%), held to go on (needs wpctl) |
| `XF86AudioMute`, `XF86AudioMicMute` | Mute the speakers, the microphone; again, unmute (needs wpctl) |
| `XF86MonBrightnessUp`, `XF86MonBrightnessDown` | Screen brightness up, down by 5%, held to go on, never quite off (needs brightnessctl) |
| `XF86AudioPlay`, `XF86AudioPause`, `XF86AudioNext`, `XF86AudioPrev`, `XF86AudioStop` | Play or pause, next, previous, stop, in the player that's playing (needs playerctl) |
| `Mod+Shift+Q` | Close the window |
| `Mod+Shift+E` | Quit Pila |
| `Mod+Shift+slash` | Mod+?: show the cheatsheet of these shortcuts, in sections (as you've bound them); any key or click hides it |
| `Mod` + left drag | Move a window from anywhere in it |
| `Mod` + right drag | Resize a window from its nearest corner |
| `Ctrl+Alt+F1` to `F12` | Switch to another virtual terminal (not nested) |

**Volume, brightness and media keys** run wpctl (PipeWire's WirePlumber), brightnessctl and
playerctl; without the program a key does nothing (and Pila says so, the first time, in a
notification). With PulseAudio, bind the volume keys to `pactl` instead. Nothing is shown on
screen by Pila; to see each change, bind the keys to an on-screen display such as swayosd
(`swayosd-client --output-volume raise`, in place of the wpctl command).

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
shows where it would go before you let go.

From the keyboard, the arrows with `Mod` step from where the window is (the action
`snap <left|right|up|down>`): `Mod+Left` fills the left half, then `Mod+Up` or `Mod+Down` takes
its top or bottom quarter, `Mod+Right` moves a quarter across to the other one, and the opposite
arrow brings a quarter back to the half. `Mod+Up` on a top quarter, or on a window that isn't
snapped, maximizes it; `Mod+Down` on a maximized window restores it. A snapped window remembers
where it was: the arrow toward the side it's already against, `Mod+Down` past the bottom, the
action `restore`, or dragging it away brings it back.

Each half and quarter is also an action of its own, for bindings that go straight there:
`snap-left`, `snap-right`, `snap-top` and `snap-bottom` (handy on a monitor turned on its side),
`snap-top-left`, `snap-top-right`, `snap-bottom-left` and `snap-bottom-right` (snapping the same
one again restores). For the arrows as they used to be, halves and maximize only:

```toml
[bindings]
"Mod+Left" = "snap-left"
"Mod+Right" = "snap-right"
"Mod+Up" = "maximize"
"Mod+Down" = "restore"
```

**Maximized and fullscreen.** `Mod+M` and `Mod+Up` maximize (the window fills the monitor, minus
any bar), `Mod+F` makes it fullscreen; pressing them again restores the window. Maximized and fullscreen windows have square corners and
no shadow.

**Gaps.** `gaps` in the configuration leaves room, in logical pixels, around maximized windows and
the halves and quarters snapped windows fill, and between those: a little desktop shows around
them, and a maximized window keeps its shadow and rounded corners, like a floating one.
Fullscreen windows still take the whole monitor. It's 0 (none) unless you set it:

```toml
gaps = 12
```

**From the keyboard.** `Mod+Shift` with an arrow moves the focused window, `Mod+Ctrl` with an
arrow resizes it (right and down grow it, left and up shrink it), and `Mod+C` centers it.

**Switching windows.** `Mod+Tab` goes to the window you used before, so a quick `Mod+Tab`
toggles between the last two. Keep Mod held and press Tab again to go further back; each window
comes forward as you reach it, and releasing Mod settles on it. Held a moment, it shows the
windows it goes through, most recently used first, in the middle of the monitor: a live preview
of each (a video keeps playing in it), with its title, the one you're on highlighted. A click on
one switches to it. `Mod+Shift+Tab` goes the other
way. `Alt+Tab` and `Alt+Shift+Tab` do the same whatever Mod is, as on other desktops, so apps
no longer get them; `"Alt+Tab" = "none"` and `"Alt+Shift+Tab" = "none"` in `[bindings]` give
them back. Clicking a
window, or a taskbar entry in your bar, focuses it too.

**The overview.** `Mod+W`, pushing the pointer into the top-left corner of the screen (not
over a fullscreen window; `[overview] hot-corner = false` turns it off) or a 4-finger swipe up
spreads the workspace's windows out, each on its own monitor, none over
another, shrunk where they don't fit, live, over the dimmed wallpaper; minimized ones are among
them, and your bar stays where it is. Click a window to go to it, or move with the arrows (the
pointer selects too) and press Enter; Escape, `Mod+W` again, the corner again, a 4-finger swipe
down or a click beside the windows close it. Any other binding closes it and does its job, but going to another workspace
(`Mod+1`..`Mod+9`, ...) shows that workspace's windows in it. With more than one workspace, a
strip on top of each monitor shows them all, live, the one you're on in the accent color: click
one to go there, or drag a window onto one to send it there (it shrinks as you drag it, to aim
easily; dropped anywhere else, it goes back to its place).

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

## Looks

What Pila draws itself, the title bars, shadows, corners and animations, is set in the
configuration file and changes as soon as the file is saved.

**Title bars and colors.** `[colors]` sets Pila's title bars: the focused window's (`bar`), the
others' (`bar-inactive`) and one asking for attention (`bar-urgent`); the title and buttons on
them (`icon`, `icon-inactive`); a button under the pointer (`hover`, `hover-icon`, and
`close-hover`, `close-hover-icon` for the close button). `background` is what shows behind the
windows without a wallpaper, and `snap-preview` the area a dragged window would snap to (it may
be translucent: `#rrggbbaa`). Apps that draw their own title bar keep their own colors. Pila's
window switcher takes the same colors; other programs can read them with `pila msg colors`.

**Wallpaper.** A wallpaper program draws it: swaybg (package swaybg), or a shell's. The default
`autostart` runs `swaybg` as it is, and Pila gives it its own wallpaper: `background`, plain,
with a small mark in the bottom-right corner, three outlined rectangles piled like windows in
greys between `background` and `hover`, the front one's top edge in the `snap-preview` color.
It's drawn in your `[colors]` (and again when they change, for swaybg's next start), and shown
whole on any monitor's shape (`-m fit`, `background` around it). For a picture of your own, give swaybg
one, with `-m` for how it covers each monitor: `fill` (the whole monitor, the picture cropped to
its shape), `fit` (the whole picture, `-c` color around it), `stretch`, `center` or `tile`:

```toml
autostart = [
    "swaybg -i ~/Pictures/wallpaper.jpg -m fill",
    "waybar",
    "mako",
    "swayidle -w timeout 300 'swaylock -f' timeout 600 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
]
```

`autostart` runs at startup: to change the picture now, `pkill swaybg` and start it again
(`swaybg -i FILE -m fill &` from a terminal). Without swaybg (a list without it, or where it
isn't installed), `[colors] background` shows. A configuration with `[wallpaper]`, which
development builds read for a while, still loads, with a warning that gives the swaybg line
doing the same.

**Rounded corners.** Windows have rounded top corners, as GTK apps round their own: 8 logical
pixels on the top two, the bottom ones square. `corner-radius` sets the radius, 0 (square) to
64, at most half the window's shorter side and, with Pila's title bar, the bar's height (28);
`rounded-corners = "all"` rounds all four instead of only the top ones (`"top"`). Maximized and
fullscreen windows stay square, and Pila leaves apps that draw their own shadow (GTK apps) to
round themselves: rounding them as well would leave a double corner. A window rule sets the
radius per app, those included. The shadow, a blur behind the window, the bell's flash and the
snap preview take the same corners.

**Shadows.** Each window casts a shadow, darker under the focused one. Maximized and fullscreen
windows have none, and neither do those whose app draws its own (GTK apps do). `[shadow]` sets
how far it reaches (`size`, 0 to 64), its `offset` and its colors (`color`, `inactive-color`,
as `#rrggbbaa`); `enabled = false` turns it off. A window rule's `shadow` turns it on or off per
app.

**Animations.** Windows opening, closing, minimizing and maximizing (snapping and fullscreen
too), launchers and notifications, workspace switches and the overview animate briefly. `[animations]`
changes them:

```toml
[animations]
speed = 2.0             # everything twice as slow (0.25 to 4)
window-close = false    # no closing animation
workspace = 150         # the workspace slide in 150 ms (1 to 1000)
# enabled = false       # no animations at all
```

A rule turns them off for an app (`animations = false`, or one kind: `animations = { open =
false }`) or for a panel or launcher. Shells that animate their own panels need that: see
[Building your desktop](#building-your-desktop).

**Blur and transparency.** An app can ask for what's behind part of it to be blurred
(ext-background-effect; some terminals and shells' panels do, and fcitx5's candidate window),
and Pila blurs it, behind menus and other popups too. A rule with `blur = true` blurs behind a
whole window (its title bar included) or panel that doesn't ask, and `blur = false` refuses an
app's request, its popups' included. Most apps' menus don't ask (GTK's don't): a window rule's
`popup-blur = true` blurs behind them all the same, and `popup-blur = false` refuses its popups
what they ask. The blur takes a menu's shape (its rounded corners, a popover's arrow) where the
app gives it, as GTK does; elsewhere it's square unless `popup-corner-radius` gives it the
menu's radius. Panels are blurred above the windows (the top and overlay layers), not below
them. A window rule's `opacity` (0.1 to 1) makes a whole window translucent, its menus included.
Rules are in [Window and layer rules](#window-and-layer-rules).

**The bell.** An app ringing the system bell (xdg-system-bell) makes its window flash briefly;
Pila plays no sound.

## Workspaces

There are four workspaces unless `workspaces` says otherwise (1 to 32). A workspace spans all
monitors: switching to another one changes what every monitor shows. Windows on all workspaces
(`Mod+P`) stay where they are.

- `Mod+1` to `Mod+9` go to a workspace, and `Mod+Shift+1` to `Mod+Shift+9` send the focused
  window there. Only as many digits as there are workspaces are taken; the others still reach
  the apps.
- `Mod+Page_Down` and `Mod+Page_Up` go to the next and previous workspace; they stop at the last
  and the first. Mod with the mouse wheel does the same, a workspace per notch (down is the
  next), and with Shift too it sends the focused window. A notch less than 150 ms after the
  last switch is dropped, so a quick turn of the wheel goes one workspace:
  `wheel-binding-cooldown` under `[input]` sets that time (0 to 1000 ms; 0 counts every notch).
- On a touchpad, four fingers left go to the next workspace and four fingers right to the
  previous one.
- Bars and pagers that support ext-workspace, such as waybar's `ext/workspaces` module, show them and switch between
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

The monitors turn off after a while without input (swayidle, in "Locking the screen and
idling"). To turn them off at once, run
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

`repeat-delay` and `repeat-rate` set how soon and how fast a held key repeats. `numlock = true`
starts Pila with Num Lock on (for a keypad's numbers), and turns it on again when a change to
`[keyboard]` loads another keymap.
`pila --check-config` tells you if xkb doesn't know a layout you wrote.

**Mouse and touchpad.** `[input.touchpad]` and `[input.mouse]` set natural scrolling, pointer
speed and acceleration, tap to click (on by default), disabling the touchpad while typing, how
a clickpad tells left from right clicks, and middle-click emulation. Settings you leave out
keep libinput's defaults for the device. Mod with the mouse wheel switches workspaces, a quick
turn of the wheel one (`wheel-binding-cooldown` under `[input]`, see [Workspaces](#workspaces)), and a binding can use a wheel notch as its key (see
[The configuration file](#the-configuration-file)).

**Touchpad gestures.** Pila handles these swipes itself:

| Swipe | What it does |
|---|---|
| `swipe-3-left`, `swipe-3-right` | Switch windows, like `Mod+Tab` and `Mod+Shift+Tab`; keep swiping for older ones |
| `swipe-3-up` | Maximize, or restore |
| `swipe-3-down` | Restore a snapped or maximized window |
| `swipe-4-left`, `swipe-4-right` | Next, previous workspace |
| `swipe-4-up`, `swipe-4-down` | Open the overview, close it |

`[gestures]` changes them, with any action a key binding can have, for 3, 4 or 5 fingers. Other
gestures (pinching, finger counts Pila doesn't use) reach the apps.

**Finding the pointer.** Lost it on a large screen, or among several? Shake the mouse (or move a
finger quickly back and forth on the touchpad): the cursor grows while you shake it, up to three
times its size, and shrinks back a moment after you stop. Moving it along a line or round a
circle doesn't count. `[cursor]` sets how big it gets, or turns it off:

```toml
[cursor]
shake-magnification = 2.0   # 1.5 to 5
# shake-to-find = false     # never grow it
```

The cursor's theme and size are `theme` and `size` under `[cursor]`, else `XCURSOR_THEME` and
`XCURSOR_SIZE` (24 unless set). Set in the file, they apply as soon as it's saved and the apps
Pila starts from then on get them as `XCURSOR_THEME` and `XCURSOR_SIZE`, so apps that draw their
own cursor (X11 ones, and older Wayland ones) match it; apps already running keep theirs until
they're started again. Apps that name their cursor shape instead (cursor-shape: GTK 4, Qt, foot,
kitty) get Pila's. Nested, the desktop around Pila draws apps' named cursors at its own size,
and the theme and size set only the grown one. Pila draws the grown cursor from the theme's larger images, so it stays sharp as far as the
theme goes. `[animations]` `speed` slows it down too, and with animations off it grows and shrinks
at once.

The mouse, touchpad and gesture settings apply from the login screen or a TTY; nested, the
desktop around Pila handles those devices. The keyboard settings apply either way.

## Input methods (Chinese, Japanese, Korean, ...)

An input method such as fcitx5 types what a keyboard layout can't: pinyin into Chinese
characters, kana into kanji, and so on. Pila works with any input method that speaks the
input-method-v2 protocol, as fcitx5 does. Start it with Pila:

```toml
autostart = ["fcitx5 -d", "waybar", "kitty"]
```

Apps reach it through the text-input-v3 protocol, with nothing to set up: GTK 3 and 4, Qt 6.7 and
later, foot and kitty. Don't set `GTK_IM_MODULE` or `QT_IM_MODULE`: fcitx5's own modules talk to it
over D-Bus instead, past Pila. Chromium and
Electron apps need `--enable-wayland-ime --wayland-text-input-version=3`. X11 apps (through
Xwayland) use XIM, which needs `XMODIFIERS=@im=fcitx` in their environment: under `[environment]`
(see [Building your desktop](#building-your-desktop)), or wherever your system sets the login
environment.

Pila's keybindings come first: a key Pila uses never reaches the input method. The candidate window
shows below the text cursor, or above it when there's no room below, clear of your panels. The lock
screen never uses the input method: what's typed there doesn't reach it.

## The configuration file

Pila reads `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila/config.toml`). The first time
it starts without one, it writes the example there, `config.example.toml`, which holds Pila's
defaults: each active (uncommented) value in it is the default, so you edit the value itself. The
commented-out lines are examples, or settings that depend on the machine (`mod`, `terminal`, the
keymap); take the `#` off one to use it.

Pila has that file built in, and takes from it every setting your file leaves out: you can delete
the lines you don't change, or keep a file with only your own settings. A setting written in your
file keeps its value when a newer Pila changes the default; one you deleted gets the new default.

A newer Pila doesn't touch the file you have. To compare it with the newest example, which
packages install in `/usr/share/doc/pila` (it's also in Pila's source), or to start over:

```bash
diff ~/.config/pila/config.toml /usr/share/doc/pila/config.example.toml
cp /usr/share/doc/pila/config.example.toml ~/.config/pila/config.toml
```

**Changes apply when you save**, all but `autostart`, which only runs when Pila starts. If the
saved file has a mistake, Pila keeps the configuration it was running with and shows a
notification with the line and what's wrong (and writes it to the log). If the file is invalid
when Pila starts, Pila starts with the defaults and says why. A file that's valid but surely
doesn't do what you meant, such as two `[[output]]` sections for one monitor (only the first
counts: put its settings in one), is used, and a notification says what's wrong.

`pila --check-config` checks the file without starting Pila, the keyboard layout included, and
prints what's wrong.

**Key bindings** go in `[bindings]`: a key combination, and an action or a command.

```toml
[bindings]
"Mod+Shift+Return" = "spawn foot -e htop"
"XF86AudioRaiseVolume" = { action = "spawn pactl set-sink-volume @DEFAULT_SINK@ +5%", repeat = true }
"XF86AudioPlay" = "none"  # to the app, as a key of its own
"Mod+J" = "none"          # remove a default binding
```

`default-bindings = false` (at the top of the file, before any table) leaves out every default
binding and gesture: only yours are left. The file Pila writes on the first run lists the defaults
under its own `[bindings]` and `[gestures]`, so there they stay yours: delete the lines you don't
want instead. That suits a scheme of your own, or Pila as the
compositor of a login screen (greetd), where Mod+Return would otherwise open a terminal before
anyone has logged in. Switching to another VT (Ctrl+Alt+F1 ...) isn't a binding, and stays.

A binding runs once each time its keys are pressed. Written as a table with `repeat = true`, as
the volume key above, it runs again while the key is held, after `repeat-delay` and at
`repeat-rate` (`[keyboard]`), until the key is released or another is pressed. Only a key can
repeat: not a wheel's notch or a modifier alone.

Keys are xkb key names (`Return`, `Tab`, `q`, `F5`, `XF86AudioMute`), or a mouse wheel's notch:
`WheelScrollUp`, `WheelScrollDown`, `WheelScrollLeft` and `WheelScrollRight` run once a notch,
but not again within `wheel-binding-cooldown` (`[input]`, 150 ms unless set), and the app under
the pointer doesn't get those notches (a touchpad's scrolling is never one).
The modifiers (`Mod`, `Shift`, `Ctrl`, `Alt`, `Super`) must match exactly. `spawn` runs the rest of the line with
`sh`, with the name of the monitor under the pointer in `PILA_OUTPUT`. The actions are listed
in `config.example.toml`.

The environment variables `PILA_MOD`, `PILA_TERMINAL` and `PILA_LAUNCHER` override `mod`,
`terminal` and `launcher`.

## Building your desktop

Pila starts the programs listed in `autostart` once it's ready (X11 apps too: the first starts
Xwayland); by default the bar `waybar`, the notification daemon `mako` and `swayidle`, and no
terminal. `autostart = []` starts nothing. The defaults are skipped, with only a line in the log,
where they aren't installed: Pila without them is a bare compositor, and the package
`pila-desktop` brings them all (see [INSTALL.md](https://github.com/igalmarino/pila/blob/main/docs/INSTALL.md#packages-for-a-full-session)). A
command you wrote yourself whose program isn't installed says so in a notification.

**Pila ships no bar, launcher or on-screen display of its own.** Its defaults are standard
programs, packaged by most distributions, each named in the default configuration alone; Pila
works the same with another in its place:

| Default | Where it's set | Swap it for |
|---|---|---|
| `fuzzel` | `launcher` | any launcher (`wofi --show drun`, `rofi -show drun`, ...) |
| `swaybg` (Pila's wallpaper), `waybar`, `mako` | `autostart` | a picture of your own (`swaybg -i FILE -m fill`), any other bar or notification daemon (a list without them) |
| `wpctl`, `brightnessctl`, `playerctl` | the volume, brightness and media keys' bindings | bindings of your own (`pactl`, `swayosd-client`, ...) |
| `grim`, `slurp`, `wl-copy` | `Mod+Shift+S` and `Mod+Ctrl+S`; `slurp` picks the screen to share, in xdg-desktop-portal-wlr's settings | bindings of your own; `chooser_cmd` in those settings |

**A look that matches Pila's.** The tools of the default configuration start with a
configuration of Pila's where you have none of your own: your `[colors]`, the title bars' font
(fontconfig's sans-serif, the one `fc-match sans-serif` names) at their size, and the windows'
8 px corners on what floats over them (mako's notifications, fuzzel, waybar's tooltips; the bar
itself stays square along the screen's edge); swaybg gets Pila's wallpaper. Its waybar has
Pila's workspaces and windows (with the apps' own icons) on the left and the tray, the volume
(a click mutes), the battery and the clock on the right, with symbols where Font Awesome is
installed (package otf-font-awesome, fonts-font-awesome or
fontawesome-6-free-fonts; `pila-desktop` brings it), words where it isn't. Pila writes nothing
where the tools look for yours: the files are in `$XDG_RUNTIME_DIR/pila-tools` (written at each
start, gone at logout), and the defaults' commands name them as they start (`waybar -c ... -s
...`, `mako -c ...`, `fuzzel --config=...`, `swaybg -i ... -m fit -c ...`, and `swaylock -C ...` in
the swayidle line), so the
same tools in another session look as before, and nothing is left behind.

This is only for the default commands as written (`waybar`, not `waybar -l debug`) and only
for a tool with no configuration of yours anywhere it reads one (`~/.config/waybar/config` or
`config.jsonc`, `~/.mako/config`, ...), checked each time it starts. To change one, copy it
from `$XDG_RUNTIME_DIR/pila-tools` (e.g. `cp -r $XDG_RUNTIME_DIR/pila-tools/waybar ~/.config/`)
and edit it there: from then on the tool reads yours. A change to `[colors]` writes the files
again: each tool takes it the next time it starts (fuzzel at once, swaybg, waybar and mako
when you log in again or restart them). `tool-configs = false` starts the tools with their own looks.

A binding of your own wins over the default one for the same keys, `"Key" = "none"` takes a
default away, and `default-bindings = false` all of them. With systemd, a program with a user service can start with the session that way
instead, restarted if it crashes and stopped when Pila quits: `systemctl --user enable
waybar.service` (in place of `waybar` in `autostart`). Everything
they open, bars, launchers, notifications and wallpapers, is placed by Pila the way those
programs ask.

**Environment variables** for everything Pila starts (`autostart`, bindings, the terminal and the
launcher) go under `[environment]`; `false` removes one Pila was started with:

```toml
[environment]
LANGUAGE = "zh_CN:zh_Hans:zh"
XMODIFIERS = "@im=fcitx"
QT_QPA_PLATFORMTHEME = false
```

A saved change reaches the programs started from then on. On a TTY the variables also go to the
D-Bus session bus and, in a login session, to systemd's user manager, so the services they start
(portals, apps started over D-Bus) get them too. The variables Pila sets itself (`WAYLAND_DISPLAY`,
`DISPLAY`, `XAUTHORITY`, `XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE`, `PILA_SOCKET`, `PILA_OUTPUT`) stay as Pila
sets them, with a warning if the section names one, and so do `XCURSOR_THEME` and `XCURSOR_SIZE`
when `[cursor]` sets them. Pila itself doesn't read the section: its own settings (such as the
keymap) are in the rest of the file.

By default the section has apps that can run on Wayland do so, with X11 through Xwayland as the
fallback: `QT_QPA_PLATFORM = "wayland;xcb"` (Qt; X11 when its Wayland plugin isn't installed),
`ELECTRON_OZONE_PLATFORM_HINT = "auto"` (Electron apps such as VS Code), `MOZ_ENABLE_WAYLAND = "1"`
(older Firefox and Thunderbird) and `_JAVA_AWT_WM_NONREPARENTING = "1"` (Java's X11 windows,
which stay blank without it). Your file's own value for one replaces it, and `false` drops it,
e.g. `QT_QPA_PLATFORM = false` for Qt apps on X11 again. `SDL_VIDEODRIVER` and `GDK_BACKEND` are
left alone: GTK picks Wayland by itself, and games that bring an older SDL fail to start with
`SDL_VIDEODRIVER = "wayland,x11"`.

**A full desktop shell** gives you the bar, launcher, notifications, wallpaper and lock screen
in one program: start it from `autostart` (in place of `waybar`), and point
`launcher` at its launcher command so Mod+D opens it instead of `fuzzel`. If it animates its own panels, turn Pila's animations off for them with a layer rule
on their namespace:

```toml
[[layer-rule]]
namespace = "my-shell-*"
animations = false
```

Shells that don't know Pila may turn monitors off on idle with `wlr-randr --off` without naming
a monitor, which fails; give their idle behavior Pila's action instead,
`pila msg action power-off-monitors`, with no resume command: any input turns the monitors back
on. Where a shell's Log out goes through logind (`loginctl terminate-session`), it works where
systemd or elogind runs it; elsewhere, or to quit Pila directly, set its command to
`pila msg action quit`. A shell's features built on a compositor's own commands (an overview,
a keyboard layout switcher) are missing until it knows Pila; Pila's own bindings still work
(a binding to `switch-layout next` switches layouts).

**With separate tools**, for example:

```toml
launcher = "fuzzel"
autostart = [
    "waybar",
    "mako",
    "swayidle -w timeout 300 'swaylock -f' timeout 600 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
]
```

**The window switcher** is drawn by Pila itself: a `Mod+Tab` or `Alt+Tab` held for a moment shows
its windows (see "Switching windows") as live previews in the title bars' colors; a quick one, there
and back, shows nothing. Another program can show the switch too, from `pila msg subscribe
switch`.

**Tried on Pila**, each started on it and seen doing its job: waybar (with the modules below),
fuzzel, swaybg, mako, swaylock, swayidle, wlsunset (night light), grim and slurp (screenshots),
wl-clipboard (`wl-copy`, `wl-paste`), wtype, wlr-randr and wlopm (monitors), fcitx5 (input
methods); Pila's tests and scripts check most of them.
Tools that use the same protocols should work too, though nobody has tried them yet: clipboard
managers (ext-data-control) and screen recorders (`wf-recorder`, screencopy).

**Lighter bars, in part:** ironbar shows the windows (`launcher`), the focused one and the clock,
but its `workspaces` module only knows a few other compositors (sway, Hyprland, ...), and its `clipboard` module wants
wlr-data-control, which Pila replaces with the standard ext-data-control. nwg-panel shows its
clock and controls, with no taskbar or workspaces (it has them for sway, Hyprland and a few others only).

**What can't work:** bars and modules that talk to another compositor's own IPC rather than a
Wayland protocol, such as dwlb (dwl's), HyprPanel (Hyprland's), waybar's `sway/*` and
`hyprland/*` modules and the like in ironbar and nwg-panel; and X11 bars such as polybar, which can't see Wayland windows.
A bar that uses standard protocols and fails on Pila is a bug: please
[report it](https://github.com/igalmarino/pila/issues).

**Waybar** shows Pila's workspaces with its `ext/workspaces` module (the active one marked, a
click switches) and the windows with `wlr/taskbar` (a click focuses one, a middle click closes
it); the configuration Pila writes has both. In a `~/.config/waybar/config.jsonc` of your own:

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

and, to see the active workspace, in `~/.config/waybar/style.css`:

```css
#workspaces button.active { background: #4a7bd0; }
```

What no protocol tells, a bar gets from `pila msg`; a waybar module that shows the keyboard
layout as it changes and switches it on a click:

```json
"custom/layout": {
    "exec": "pila msg --json subscribe keyboard | jq --unbuffered -r '.layouts[]? | select(.active) | .name'",
    "on-click": "pila msg action switch-layout next"
}
```

**Programs started by Pila** get `XDG_CURRENT_DESKTOP=pila` and `PILA_SOCKET`, for `pila msg`.
Programs such as kitty open links with `xdg-open`, from the `xdg-utils` package: without it, a
click on a link does nothing.

**Notifications from Pila** (an invalid configuration, for instance) are sent over D-Bus and
shown by your notification daemon (mako, your shell's, ...): no other program is needed. Without
a daemon they only reach the log.

## Screenshots and screen sharing

`Mod+Shift+S` lets you drag a region (with slurp) and puts it on the clipboard as a PNG image;
`Mod+Ctrl+S` saves the monitor under the pointer to `~/Pictures` as
`screenshot-YYYYMMDD-HHMMSS.png`. They need `grim`, and the first also `slurp` and
`wl-clipboard`. `grim` and other screenshot tools work on their own too, and so do a shell's own
screenshot features.

**Screen sharing** in browsers, OBS and video calls goes through the desktop portal
(`xdg-desktop-portal`), which hands it to Pila: Pila asks what to share and streams it over
PipeWire, with no other program. When an app asks to share, a panel in the middle of the screen
names the app and shows what it may share as live previews: every monitor and, if the app shares
windows too, every window, most recently used first (minimized ones and those on other workspaces
too), under **Screens** and **Windows**. The monitor under the pointer is selected first (the
window you used last, for an app that shares only windows): click a preview or use the arrows
to pick one, then **Share** (or Enter) to share it, or **Cancel** (or Escape). Nothing is shared
until then, and while the panel is shown no key or click reaches an app. A shared window is
streamed alone, at its size, and keeps updating while it's covered, minimized or on another
workspace; while the screen is locked its stream shows nothing new, and closing the window ends
the share. The pointer shows in the share as the app asks (browsers draw it themselves), over a
window only while it's over it. Locking the screen cancels the question. Pila installs the
portal's configuration for a Pila session (`pila.portal` and `pila-portals.conf`); it needs
`xdg-desktop-portal` and `pipewire`.

Screen sharing needs the session's D-Bus bus, which the login screen and `pila-session` set up;
a nested Pila doesn't have it.

## Portals, polkit and the keyring

Apps ask the session for a few things a full desktop starts for them: a file chooser and other
dialogs (the desktop portal), a password prompt for administrator tasks (a polkit agent), and a
place to keep the passwords they save (a keyring). Pila draws none of them; standard programs
do. On a TTY Pila hands the session bus and systemd its environment by itself (`WAYLAND_DISPLAY`,
`DISPLAY`, `XAUTHORITY`, `XDG_CURRENT_DESKTOP=pila` and `[environment]`'s), so nothing like
`dbus-update-activation-environment` is needed in `autostart`.

**Portals.** xdg-desktop-portal picks its backends from `pila-portals.conf` (named after
`XDG_CURRENT_DESKTOP`). Pila's, in `/etc/xdg/xdg-desktop-portal/`, uses `xdg-desktop-portal-gtk`
for everything (a GTK file chooser) but screen sharing (Pila itself) and screenshots
(`xdg-desktop-portal-wlr`). For GNOME's file chooser, install `xdg-desktop-portal-gnome` and
Nautilus, and write your own `~/.config/xdg-desktop-portal/pila-portals.conf`. It replaces Pila's
whole, so keep its `ScreenCast` and `Screenshot` lines, or screen sharing stops working:

```ini
[preferred]
default=gnome;gtk
org.freedesktop.impl.portal.ScreenCast=pila
org.freedesktop.impl.portal.Screenshot=wlr
# These need GNOME Shell in the GNOME backend.
org.freedesktop.impl.portal.Access=gtk
org.freedesktop.impl.portal.Notification=gtk
org.freedesktop.impl.portal.Inhibit=gtk
```

Log in again, or `systemctl --user restart xdg-desktop-portal`. Qt apps use the portal's file
chooser with `QT_QPA_PLATFORMTHEME = "xdgdesktopportal"` under `[environment]`; Firefox with
`widget.use-xdg-desktop-portal.file-picker` set to `1` in `about:config`. GTK 4 apps follow the
dark style through the portal: `gsettings set org.gnome.desktop.interface color-scheme
prefer-dark` (either backend reads it).

**Polkit.** An app that needs an administrator's password (`pkexec`, a disk utility, a software
center) asks a polkit agent for it, which a desktop starts in its session. Pila doesn't, and
`pila-desktop` doesn't bring one: without one, those apps fail with no prompt. Start one from
`autostart`, as the last of your list (an `autostart` of yours replaces the default one, so keep
the default's programs in it), e.g. GNOME's:

```toml
autostart = [
    "swaybg",
    "waybar",
    "mako",
    "swayidle -w timeout 300 'swaylock -f' timeout 600 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
    "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
]
```

Its path depends on the distribution (that one is Arch's; package polkit-gnome).
`lxqt-policykit-agent` and `mate-polkit` work too; run one agent at a time.

**The keyring.** Passwords apps save (browsers, mail, Git's credential helper over libsecret)
go to a Secret Service: GNOME Keyring, or KeePassXC with its Secret Service on. GNOME Keyring
unlocks with your login password through PAM: in the login screen's PAM file
(`/etc/pam.d/greetd` for greetd), `auth optional pam_gnome_keyring.so` and `session optional
pam_gnome_keyring.so auto_start`. It starts when an app first asks for it (D-Bus activation);
where it doesn't, add `gnome-keyring-daemon --start --components=secrets` to `autostart`.

## Locking the screen and idling

Lock screens that use ext-session-lock work: `swaylock`, or your shell's own.
While the screen is locked, only the lock screen gets the keyboard and the mouse, and Pila's
bindings don't run; `Ctrl+Alt+F1` to `F12` still switch virtual terminals.

What happens without input is swayidle's job, which the default `autostart` runs: it locks the
screen with swaylock after 5 minutes without input (a key, a click, a move of the pointer) and
before the computer sleeps, and turns the monitors off after 10. Any input turns them back on,
and that first key or click does nothing else. A video playing in a visible window keeps the
session from idling, if the player asks for it, as most do. To change the times, or not lock,
put your own swayidle line in `autostart` (it's read at startup: log in again, or start swayidle
again yourself):

```toml
autostart = [
    "waybar",
    "mako",
    # Lock after 15 minutes, monitors off after 20, lock before sleeping.
    "swayidle -w timeout 900 'swaylock -f' timeout 1200 'pila msg action power-off-monitors' before-sleep 'swaylock -f'",
]
```

`man swayidle` has the rest (`idlehint`, `lock` and `unlock` from logind). Locking before the
computer sleeps (`before-sleep`) needs logind (systemd or elogind); the timeouts work without
it. A shell with its own idle settings takes swayidle's place: leave it out of `autostart`.
Monitors that `pila msg action power-off-monitors` turned off come back at any input; `wlopm
--off '*'` works too, with a `resume` that runs `wlopm --on '*'`, as monitors a tool turned off
stay off until it turns them on.

A configuration with `[idle] monitors-off`, which development builds read for a while, still
loads, with a warning that gives the swayidle line doing the same.

If the lock screen crashes, the session stays locked, for safety. Switch to a free virtual
terminal (`Ctrl+Alt+F3`, or another free one: F1 and F2 may be the login screen or this very
session), log in and start a new lock screen on Pila's display, for instance
`WAYLAND_DISPLAY=wayland-1 swaylock` (`ls $XDG_RUNTIME_DIR` shows the name); unlocking it gives
you the session back.

## Games and X11 apps

X11 apps (older programs, many games, Steam) run through Xwayland, which Pila starts by itself
when the first X11 app opens: `DISPLAY` is set from the start, but a session with no X11 app
runs no X server. Only apps with the X server's cookie get in: Pila gives it to what it starts as
`XAUTHORITY`, a file in `$XDG_RUNTIME_DIR` only you can read, made anew each session (Flatpak
apps with X11 permission get it through Flatpak). The clipboard works between X11 and Wayland
apps. Games can lock and confine the pointer.

Once started, Xwayland runs until Pila exits. To have it stop when no X11 app is left, set a
timeout in seconds; the next X11 app starts it again:

```toml
[xwayland]
idle-timeout = 10
```

Pila stops it only when no X client is connected at all, so a background X program (a settings
daemon, an input method's X part) keeps it running. Stopping it loses whatever was set in the X
server itself (`xrdb`, `setxkbmap`, `xmodmap`): leave the timeout unset if your autostart sets
any of those.

For a session with no X11 at all, turn it off; Pila then reserves no X display and sets no
`DISPLAY` nor `XAUTHORITY`, so apps that can use either pick Wayland:

```toml
[xwayland]
enabled = false
```

Turned off while X11 apps run, Xwayland stays until they're gone and it stops (with
`idle-timeout`) or Pila exits; programs started meanwhile get no `DISPLAY`. Xwayland is an
optional dependency of Pila's packages, and without it installed there are no X11 apps either.

Apps that ask for every key while they have the focus (virtual machines, remote desktops:
keyboard-shortcuts-inhibit, or an X11 app that grabs the keyboard) get Pila's bindings and
touchpad gestures too, all but the one that quits Pila. Clicking another window always gets out.

X11 launchers like dmenu (`launcher = "dmenu_run"`) get the keyboard when they open, even over a
Wayland window, and give it back when they close; X11 menus and tooltips leave it where it is.

A rule turns off the window animations for Steam games:

```toml
[[window-rule]]
app-id = "steam_app_*"
animations = false
```

On monitors that support it, `adaptive-sync = true` in an `[[output]]` section turns on variable
refresh rate.

A fullscreen game or video that is all a monitor shows, with nothing over it, goes to the screen
as the app drew it: Pila draws nothing for those frames, which saves GPU work and battery
(direct scanout; on a TTY, on the main GPU's monitors, when the graphics driver takes the app's
picture as it is). If a fullscreen window flickers or its colors look wrong,
`direct-scanout = false` in the configuration turns it off as soon as the file is saved.

A game with vsync off (in its own settings, or Vulkan's immediate present mode) asks for its frames
to be shown at once rather than at the monitor's next refresh: less latency, at the price of
tearing, a frame showing partly over the last one. Pila does so while the game is fullscreen and
nothing but it changes on that monitor (the pointer moving over it waits for the refresh), on a
TTY, when the graphics driver takes it. With vsync on in the game, nothing tears. `tearing = false`
in the configuration turns it off for every game.

On a TTY Pila draws each frame as close to the monitor's next refresh as it safely can, rather
than right after the last one: what an app draws and where the pointer goes in the meantime still
make that refresh, a refresh sooner than otherwise (less latency in games with vsync on, video,
scrolling). How close follows how long the last few seconds of frames took, CPU and GPU, plus
some room; if a frame comes late anyway, Pila draws at once on that monitor for a couple of
seconds. It does so only while the screen is steady: not during Pila's animations, nor for a
moment after a window or panel opens or a window changes size (its app busy on the GPU), nor
while a game tears or with adaptive sync. `render-late = false` in the configuration turns it
off.

## Window and layer rules

Rules change how particular windows or panels behave. A `[[window-rule]]` matches windows by
app id, title or tag, with `*` for any text, and can set: animations, the workspace and monitor
to open on, opening without the keyboard focus, size, position, opening maximized or
fullscreen, staying above the others or on all workspaces, the title bar, corner radius,
opacity, blur (its own and its menus') and shadow. A window's rules follow its
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

A new window opens on the monitor under the pointer. `open-on-output` names another, by
connector (`eDP-1`) or as an `[[output]]` section does (`"Make Model"`); if that monitor isn't
plugged in or is off, the pointer's is used. With `open-focused = false` the window opens on top
but the keyboard stays with the window you were typing in. Together they keep a window that opens
while you work out of the way: Pila run nested for development, say, whose window has the app id
`pila`:

```toml
[[window-rule]]
app-id = "pila"
open-on-output = "eDP-1"
open-focused = false
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
pila msg minimize <id>           # minimize a window (focus brings it back)
pila msg switch                  # the windows of a Mod+Tab in progress
pila msg --json subscribe        # windows, workspaces and monitors as they change
```

Scripts should use `--json`: the plain text is for reading and may change. A bar or script that
follows the windows or workspaces subscribes instead of asking over and over. The full
description, the socket and the events included, is in [IPC.md](https://github.com/igalmarino/pila/blob/main/docs/IPC.md).

## Troubleshooting

**The log** is in `~/.local/state/pila/pila.log` (`$XDG_STATE_HOME/pila` if that's set; the run
before is in `pila.log.old`), and with
systemd also in the journal: `journalctl -t pila -b`. `RUST_LOG=debug` makes it detailed (run
`RUST_LOG=debug pila-session` from a TTY, or `RUST_LOG=debug scripts/tty` from Pila's source).

**`pila-report`** gathers what a bug report needs (version, system, GPU, monitors, whether the
configuration is valid, the end of the log) into `pila-report.txt`, without your user name,
host name and home directory. It sends nothing anywhere; read it before posting it.

**`pila-check`** tests Pila on your hardware: run from a TTY (Ctrl+Alt+F3, log in) while your
session keeps running on its own, it starts Pila with your monitors, keyboard and input
settings, runs real programs through windows, workspaces, captures, blur and fullscreen for
about a minute, quits by itself and prints PASS or FAIL per check, with frame times (CPU and GPU)
and each monitor's latency. Its log is `~/.local/state/pila/pila-tty.log`. `pila-check --nested`
does the same in a window of your session, `PILA_CHECK_RENDER_LATE=false pila-check` runs it with
`render-late` off to compare, and `pila-check --help` lists the rest.

| Problem | What to do |
|---|---|
| The Pila session doesn't start | Pick another session at the login screen and read the log. `pila --check-config` rules out the configuration. |
| A notification says the configuration is invalid | It gives the line and the problem; fix it and save. `pila --check-config` checks it again. A key binding must be under `[bindings]`. |
| A change to the configuration does nothing (another setting stopped working too) | The file is invalid, and Pila keeps the last valid one; the notification saying so needs a notification daemon. `pila --check-config` shows the line and the problem. |
| Holding a volume or brightness key changes it only once | Write its binding as a table with `repeat = true`: `"XF86AudioRaiseVolume" = { action = "spawn wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+", repeat = true }`. |
| The volume, brightness or media keys do nothing | Their defaults run `wpctl` (package wireplumber, PipeWire's), `brightnessctl` and `playerctl`: install them, or bind the keys to your own commands (`pactl` on PulseAudio). The media keys need a player that speaks MPRIS. `default-bindings = false` leaves them out. Pila tells you, the first time, when a key's program isn't installed (a notification: "brightnessctl isn't installed"); any binding, the terminal, the launcher or `autostart` too, except the default `autostart`'s own tools, skipped quietly. |
| `Mod+Shift+S` or `Mod+Ctrl+S` does nothing | They need `grim`, and `Mod+Shift+S` also `slurp` and `wl-clipboard` (`wl-copy`): Pila tells you when one of them isn't installed. |
| Clicking a link in the terminal does nothing | Install `xdg-utils`. |
| No notifications from Pila | Run a notification daemon (mako, or your shell's) on the session bus; the log says once when there's none. |
| An app's administrator password prompt never shows (`pkexec` fails at once) | No polkit agent runs: start one from `autostart` ([Portals, polkit and the keyring](#portals-polkit-and-the-keyring)). |
| Screen sharing shows nothing, or no panel asks which screen | Install `xdg-desktop-portal` and `pipewire`, and start Pila from the login screen or `pila-session`. A `~/.config/xdg-desktop-portal/pila-portals.conf` of your own whose `ScreenCast` line says `wlr` hands sharing to `xdg-desktop-portal-wlr` instead (its configuration in `~/.config/xdg-desktop-portal-wlr/pila`, if it's an older copy, runs `pila-shot`, which Pila no longer has: change its `chooser_cmd` to `slurp -f 'Monitor: %o' -or`). |
| The shell doesn't turn the screens off | Some shells run `wlr-randr --off` without a monitor name, which fails; use `pila msg action power-off-monitors` (any input turns them back on). |
| X11 apps don't start | Install Xwayland; the log says if it couldn't start. |
| Title bars have no text | Install a font (any TrueType or OpenType font). |
| Stuck on a locked screen after the locker crashed | See [Locking the screen and idling](#locking-the-screen-and-idling). |
| `Mod+D` or tapping Mod does nothing | `launcher` is `""`, or names a launcher that isn't installed (by default `fuzzel`): install it, or set `launcher` to one that is (see [Building your desktop](#building-your-desktop)). |
| No bar, or no terminal, at login | Pila starts only what `autostart` lists, no terminal by itself (Mod+Return opens one): install `waybar` (the default) or list your bar, and any terminal you want, there. |
| An app needs Mod with the mouse wheel, which switches workspaces | `"Mod+WheelScrollDown" = "none"` and `"Mod+WheelScrollUp" = "none"` under `[bindings]` give it back (nested, Mod is Alt). |
| Tapping Mod opens the launcher and you don't want it | `"Mod" = "none"` under `[bindings]` (nested, Mod is Alt, so tapping Alt opens it). |
| A fullscreen window flickers or shows wrong colors | `direct-scanout = false` in the configuration (Pila then draws those frames itself), and report it with `pila-report`'s file. |
| The screen froze | `Ctrl+Alt+F3` (any free virtual terminal), log in, `pkill pila`; then report it with the log. |
| Pila crashed (back at the login screen) | Report it with `pila.log.old` (or `pila-report`'s file): its "pila crashed" line and the backtrace after it say where. |

Bugs go to the [issues](https://github.com/igalmarino/pila/issues), with `pila-report`'s file
or the log; questions and ideas to the
[discussions](https://github.com/igalmarino/pila/discussions).
