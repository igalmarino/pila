# Changelog

## Unreleased

- **Share a window:** an app that may share windows (browsers, video calls, OBS) gets them in
  Pila's panel too, under the monitors: every window, most recently used first, minimized ones
  and those on other workspaces too, each as a live preview. The window is streamed alone at its
  size, keeps updating while it's covered, minimized or on another workspace, follows its size,
  shows nothing new while the screen is locked, and its share ends when it closes. No more need
  of xdg-desktop-portal-wlr to share a single window.
- **Apps' screenshots served by Pila:** an app that asks the desktop portal for a screenshot (a
  browser's, a Flatpak's) gets it from Pila. With the portal's permission to take one unasked it
  gets every monitor at once, as they're laid out; otherwise, or when it wants you to choose, the
  share panel asks what to take, a monitor or a window (the window alone, even covered or
  minimized), with Take. The picture, without the pointer, is saved in
  `$XDG_RUNTIME_DIR/pila-screenshots` (the last ten kept) and handed to the app. Pila's portal
  configuration now names Pila for screenshots instead of xdg-desktop-portal-wlr; an own
  `pila-portals.conf` should say `org.freedesktop.impl.portal.Screenshot=pila`.
- **The share panel names apps outside a sandbox** by the window they ask from (most browsers
  say which): its app's name instead of "An app asks to share a screen".
- **No more xdg-desktop-portal-wlr:** with screen sharing, single windows and apps' screenshots
  Pila's own, the packages no longer recommend it and Pila no longer installs its settings
  (`/etc/xdg/xdg-desktop-portal-wlr/pila` is removed; `scripts/tty` no longer writes
  `~/.config/xdg-desktop-portal-wlr/pila`, and leaves an older copy alone). It can still be used
  through a `pila-portals.conf` of your own.
- **Pick a color from the screen:** apps that ask the portal for one (GTK's color chooser, GIMP,
  Inkscape) get a crosshair and a panel saying what to do; a click answers with the color under
  it, Escape gives up, and no app gets the click.
- **`scripts/install` names what a source build is missing:** it checks that pkg-config, the
  libraries' development files and libclang are there before it builds, and lists what to
  install, instead of a build script's panic deep in the build (bindgen's, for libclang).

## 0.9.0 (2026-11-22)

- **Screen sharing served by Pila:** browsers, OBS and video calls now share the screen through
  Pila itself, Pila's own backend of the desktop portal (`pila.portal`), over PipeWire: no
  xdg-desktop-portal-wlr for it any more (it stays for screenshots). When an app asks, a panel
  names it and shows every monitor as a live preview; click or arrows to pick, Share or Enter,
  Cancel or Escape. Nothing is shared until then; the panel is modal, and locking the screen
  cancels it. Frames are drawn straight into the app's GPU buffers when it takes them (dma-bufs:
  about 0.2 ms of Pila's time per frame at 1080p), else read back into shared memory; the
  pointer shows as the app asks (drawn by browsers themselves), and a share follows its monitor
  turned or scaled. Pila now needs `libpipewire` (and libclang to build); a single window is
  still shared through xdg-desktop-portal-wlr (the manual says how).
- **Pila's panels look alike:** the window menu is drawn as the switcher, the cheatsheet and the
  share picker are (their background, border and corners, the row under the pointer on the
  accent); the panels with a title share its font and place and end in a row of key hints; the
  cheatsheet and the welcome dim every monitor while they're shown, as the share picker does.
- **Fixed:** the window menu is drawn in front of the shortcuts cheatsheet shown while it's open
  (by `pila msg action show-shortcuts`, say), not behind it.
- **`pila-check` judges captures' copies** into shared memory too (`copies` in the frame
  statistics, within 8 ms each): they hold the event loop as uploads do.

## 0.8.3 (2026-11-22)

- **Gaps:** `gaps` leaves room around maximized windows and the halves and quarters windows are
  snapped to, and between snapped ones, in logical pixels (0, none, by default). With room
  around it a maximized window keeps its shadow and rounded corners; fullscreen windows still
  take the whole monitor.
- **Windows no longer end up under a bar that comes after them:** a window snapped to a half or
  a quarter stayed where it was when a bar appeared or grew (an app snapped at login, before
  waybar was up), its top under the bar; it now moves below it, as maximized windows already
  did. And a maximized window refit to a bar no longer comes in front of the other windows.
- **No refresh missed when a fullscreen shm app opens:** a client's buffer drawn with the CPU
  (foot, swaybg's wallpaper, Xwayland apps without GPU acceleration) is now copied to the GPU in
  strips, not all at once: on AMD's driver a whole 1920x1080 buffer in one copy took 10 to 22 ms
  about one time in three, and a fullscreen foot's first buffer made the screen miss a refresh;
  in strips the worst copy takes under 5 ms. `pila-check` now judges each copy (within 8 ms).
- **Why a fullscreen app isn't shown as it is** (direct scanout, bare metal) is logged on
  `pila::frames`: the format and modifier of a buffer the screen refused, and whether its app
  was told the screen's formats. With Mesa 26.3 on AMD, OpenGL apps keep a buffer layout the
  screen can't read, so their frames are drawn by Pila (docs/ROADMAP.md).

## 0.8.2 (2026-11-21)

- **Security: X11 apps can no longer be reached from a sandbox:** the X display Pila reserves
  for Xwayland listened on an abstract socket too, which has no permissions and needs no
  authorization, so a Flatpak app with network access but no X11 permission (or another user
  on the machine) could connect to it: replace the clipboard, read it while an X11 app had the
  focus, and type into X11 apps. It now listens only on its socket file in `/tmp/.X11-unix`.
- **Security: X11 apps need Xwayland's cookie:** Xwayland now runs with `-auth`, and only
  clients with its cookie get in: programs Pila starts get it as `XAUTHORITY` (a file in
  `$XDG_RUNTIME_DIR` only the user can read, made anew for each session), and so does the
  D-Bus session bus on a TTY. A process that can reach the X socket without it (another user, a
  sandbox without X11 permission) is refused.
- **Security: no focus stealing by hovering:** an app could take the keyboard focus (and what
  was typed next, such as a password in a terminal) whenever the pointer merely rested on one of
  its windows, by asking for an activation token with the serial of the pointer entering it.
  Only a click on it counts now, and only until the keyboard focus moves elsewhere.
- **Blur behind any app's menus:** a window rule's `popup-blur = true` blurs behind its menus
  and other popups even when the app doesn't ask (GTK's menus don't), in the shape the app gives
  them (GTK's rounded corners and popover arrows) and not behind the shadow it draws around
  them; for apps that don't give it, `popup-corner-radius` rounds that blur to the menu's own
  corners. With `opacity` in the same rule, the window and its menus are both translucent and
  blurred. `popup-blur = false` refuses popups the blur they ask for.
- **Smoother with apps that draw in shared memory:** a picture an app draws with the CPU
  (swaybg's wallpaper, foot, Xwayland apps without GPU acceleration) is now copied to the GPU as
  the app sends it, not in the middle of the frame that shows it. The wallpaper's first frame
  took up to 19 ms on a laptop and was shown a refresh late at every start; it now takes under
  4 ms. A fullscreen foot scrolling on a 2560x1440 monitor costs Pila about half as much per
  frame. The copies' time is logged apart (`uploads` on `pila::frames`) and reported by
  `pila-check`, which also runs a fullscreen foot where it's installed.
- **For packagers: zbus 6.** Pila's D-Bus crate moves up a major version, which takes in the
  `zvariant` and `zbus_names` crates `cargo audit` reported unmaintained (RUSTSEC-2026-0370 to
  0372): 14 fewer crates to build. No new system library.

## 0.8.1 (2026-11-20)

- **An overview:** **Mod+W** (the new action `overview`; `overview open` and `overview close`
  do only one), the top-left corner of the screen (`[overview] hot-corner`) or a 4-finger swipe
  up (down closes it) spreads the workspace's windows out on their monitors, none over another, shrunk
  where they don't fit, live, over the dimmed wallpaper, minimized ones included; the bar stays.
  A strip on top shows every workspace, live; click one to go there, or drag a window onto one
  to send it there.
  Click a window to go to it, or select one with the arrows (or the pointer) and press Enter;
  Escape, Mod+W again or a click beside the windows close it. Going to another workspace shows
  that one's windows in it; any other binding closes it first. The cheatsheet lists it. The
  windows glide to their places and back (`[animations] overview`, 250 ms).
- **Fixed:** on bare metal, a game that tore (vsync off, fullscreen) and then left fullscreen
  made the frame statistics count as many missed refreshes as it had torn frames' vblanks
  (`scripts/check` and `pila::frames`): the frame after the last torn one was judged against
  the vblank before tearing started.

## 0.8.0 (2026-11-20)

- **Rounded corners as GTK's:** windows Pila decorates now have their top corners rounded by
  8 logical pixels, as GTK apps round their own windows, so a terminal next to a GTK app looks
  the same; the bottom corners stay square. The new `rounded-corners = "all"` rounds all four
  (as before, with `corner-radius`), and `corner-radius = 0` keeps them square. A configuration
  written by an earlier Pila has `corner-radius = 0` of its own: delete the line to take the new
  default. A blur behind an app that rounds its own corners (libadwaita's, on all four) is
  rounded on all four too.
- **A wallpaper of Pila's:** the default `autostart` runs `swaybg`, and Pila gives it its own
  wallpaper: `[colors] background`, plain, with a small mark in the bottom-right corner (three
  outlined rectangles piled like windows, in greys just above the background, the accent along
  the front one's top edge), drawn in your `[colors]` (again when they change, for swaybg's next
  start) and shown whole on any monitor's shape. For a picture of your own,
  put `swaybg -i FILE -m fill` in its place. A configuration written by an earlier Pila has an
  `autostart` of its own: add `"swaybg"` to it. `pila-desktop` now brings swaybg.
- **A welcome screen and a cheatsheet:** the first time Pila starts, when it writes your
  configuration file, it welcomes you with the few keys to get going (a terminal, the launcher,
  switching and closing windows, quitting), as keycaps, as they're bound (your Mod key, your own
  keys; an action you unbound isn't listed). Any key or click hides it, and a binding still runs
  (Mod+Return hides it and opens a terminal). **Mod+?** (`Mod+Shift+slash`, the new action
  `show-shortcuts`) shows all the shortcuts at any time, in sections: windows, workspaces, the
  mouse and Pila. Both panels have the windows' corners.
- **`pila-desktop`, or the compositor alone:** a new metapackage (APT, DNF, the AUR and the
  flake) brings Pila with every tool its default configuration runs: kitty, waybar, fuzzel,
  mako, swayidle and swaylock, Font Awesome, and the volume, brightness, media and screenshot
  keys' programs. `pila` itself no longer recommends them, only what apps want in any session
  (Xwayland, the portals, xdg-utils), so installing `pila` gives the compositor alone; on
  Debian and Fedora, install `pila-desktop` to keep the tools an earlier `pila` brought in
  (`apt autoremove` or `dnf autoremove` would otherwise take them away). The default
  `autostart`'s own commands (waybar, mako, swayidle) are skipped, with a line in the log,
  where they aren't installed, rather than reported in a notification; a command of your own
  that isn't installed is still reported.
- **The screen locks and the monitors turn off by themselves:** the default `autostart` runs
  swayidle, which locks the screen with swaylock after 5 minutes without input and before the
  computer sleeps, and turns the monitors off after 10 (`pila msg action power-off-monitors`:
  any key, click or move of the pointer turns them back on, and that first key or click does
  nothing else). A video playing in a visible window keeps the session from idling. A
  configuration with its own `autostart` adds the swayidle line from the example or the manual;
  a shell with its own idle settings needs none. `pila-desktop` brings swayidle and swaylock.
- **Standard tools in place of Pila's own:** `pila-launcher` and `pila-bar` are gone, so that
  Pila is the compositor alone; the default `launcher` is now fuzzel and the default `autostart`
  waybar, both packaged by every major distribution (`pila-desktop` brings them, and the
  manual's waybar configuration shows Pila's workspaces). A configuration that still names
  `pila-launcher` or `pila-bar` gets the notification that it isn't installed: put fuzzel and
  waybar, or any launcher and bar, in their place. `scripts/install` and `scripts/uninstall`
  remove the binaries an older install left. The screenshot keys keep running grim, slurp and
  wl-copy, and screen sharing keeps picking the monitor with slurp.
- **One look for the tools:** the default configuration's tools (waybar, mako, fuzzel, and
  swaylock from the swayidle line) start with a configuration of Pila's where you have none of
  your own, so a session looks like one desktop: your `[colors]` (a change reaches each tool at
  its next start), the title bars' font at their size, and the windows' 8 px corners on what
  floats (notifications, fuzzel, waybar's tooltips; the bar stays square). waybar shows the
  apps' own icons in its window list, and symbols for the volume, battery and clock where Font
  Awesome is installed (`pila-desktop` brings it), words where it
  isn't. Pila writes nothing in `~/.config`: the files are in `$XDG_RUNTIME_DIR/pila-tools`
  and the default commands name them as they start (`waybar -c ...`), so the same tools in
  another session keep their looks; copy one to `~/.config` to change it. fuzzel is sized by the
  monitor's scale, as Pila is, so it's the same size on a laptop's panel and a monitor beside it.
  `tool-configs = false` turns it off. mako, the notification daemon, joins waybar in the
  default `autostart`.
- **Title bars in the system's font:** title bars and the window switcher draw in fontconfig's
  sans-serif (`fc-match sans-serif`), as GTK apps and the tools do; before, they asked for Open
  Sans, and got whichever font came first where it isn't installed.
- **Fewer programs to install:** Pila sends its notifications over D-Bus itself, so `notify-send`
  (libnotify) is no longer needed, only a notification daemon such as mako; a daemon started
  after Pila still gets the next notification. `pila-report` names the GPU from `/sys` and the
  system's `pci.ids`, without `lspci`, so the packages no longer suggest pciutils. On a TTY,
  Pila gives the session bus and `systemd --user` its environment over D-Bus too, without
  `dbus-update-activation-environment` (which some distros package apart, such as Fedora's
  dbus-tools), and again when a reload changes `[cursor]`'s theme or size.
- **A key whose program isn't installed says so:** when a command Pila runs (a binding, the
  terminal, the launcher, `autostart`) fails because a program it needs isn't installed, a
  notification names it ("wpctl isn't installed") and the command, once a session per
  program, where before only the log said so and the key did nothing.
- **The volume, brightness and media keys work out of the box:** default bindings for the volume
  keys and mute (wpctl, PipeWire), the microphone's mute key, the brightness keys (brightnessctl,
  never quite off) and play, pause, next, previous and stop (playerctl), the volume and
  brightness ones repeating while held. They come with a configuration Pila wrote before too
  (defaults are added to the file's bindings, unless `default-bindings = false`); your own
  bindings of those keys still win. Without the program a key does nothing; `pila-desktop`
  brings all three.
- **A window switcher you can see:** held a moment, Mod+Tab or Alt+Tab shows the windows it
  goes through in the middle of the monitor, most recently used first: a live preview of each (a
  video keeps playing in it, minimized windows and those on another monitor too), with its
  title, the one it's on highlighted; a click on one switches to it. A quick Mod+Tab, there and
  back, shows nothing. Pila draws it itself, with the windows' corners: nothing to start or
  install, and the previews are the windows' own pictures, with no capture.
- **The window switch over IPC:** `pila msg switch` and the `switch` event (`pila msg subscribe
  switch`) tell the windows of a Mod+Tab or Alt+Tab in progress, most recently used first, and
  the one it's on, at each step, and null when it ends: for a program that shows it.
- **`pila msg colors`:** lists `[colors]`, and `pila msg subscribe colors` follows them as the
  configuration is saved (a new request and event in the IPC), for programs that want to look
  like Pila's title bars.
- **Fix: a nested Pila quits when its host goes.** When the session it ran in as a window ended
  or crashed, it ran on with no window, printing `Io error: Broken pipe` nonstop; it now quits
  with "lost the host compositor".
- **APT and DNF repositories:** Pila's `.deb` and `.rpm` are now in signed repositories on
  GitHub Pages, so Debian, Ubuntu and Fedora install it once and update it with the system
  (README, "Debian, Ubuntu and Fedora"). The `.rpm` is built on the oldest Fedora maintained
  (44, glibc 2.43, which it now requires) instead of the newest, so one serves every Fedora
  maintained.

## 0.7.1 (2026-11-18)

- **The arrows step through halves and quarters:** `Mod+Left` fills the left half, then
  `Mod+Up` or `Mod+Down` takes its top or bottom quarter, `Mod+Right` moves a quarter across,
  and the opposite arrow comes back to the half; toward the side a window is already against
  restores it, `Mod+Up` on a top quarter maximizes. From a window that isn't snapped the arrows
  do what they did (halves, maximize, restore a maximized one). It's the new action
  `snap <left|right|up|down>`; a configuration written by an earlier Pila keeps its arrows as
  they were, and the manual shows how to get them back.
- **A systemd session:** with systemd, `pila-session` runs Pila as the user service
  `pila.service`, bound to `graphical-session.target`, so the user services of the graphical
  session (`WantedBy=graphical-session.target`) and the XDG autostart entries
  (`~/.config/autostart`) start once Pila is ready and stop when it quits, and Pila's variables
  leave systemd's environment after the session. A second login while one runs is refused.
  `PILA_NO_SYSTEMD=1` keeps the old way; without systemd nothing changes. The packages install
  the units in `/usr/lib/systemd/user` (`scripts/install`: `SYSTEMDUSERUNITDIR`).
- **`[environment]`:** environment variables for everything Pila starts (autostart, bindings, the
  terminal and the launcher), `NAME = "value"`, or `NAME = false` to remove one Pila was started
  with; a saved change reaches what starts from then on. On a TTY they also go to the D-Bus
  session bus and systemd's user manager. Pila's own variables (`WAYLAND_DISPLAY`, `DISPLAY`, ...)
  stay as Pila sets them, with a warning if the section names one. By default it has Qt,
  Electron, older Firefox and Java's X11 windows run on Wayland (or as they should under
  Xwayland): `QT_QPA_PLATFORM = "wayland;xcb"`, `ELECTRON_OZONE_PLATFORM_HINT = "auto"`,
  `MOZ_ENABLE_WAYLAND = "1"`, `_JAVA_AWT_WM_NONREPARENTING = "1"`; `NAME = false` drops one.

## 0.7.0 (2026-11-18)

- **`pila-launcher`, an app launcher of Pila's own:** the installed apps (their `.desktop`
  files) with their icons, searched as you type; Return or a click starts one, Escape closes
  it. It's the default `launcher`, so Mod+D and tapping Mod open it; apps that run in a terminal start in kitty, or `--terminal`
  names another. It stays in the background after the first use, so it opens at once; input methods
  such as fcitx5 type into it. Written with iced, rendered in software: it needs no new library,
  and the packages include it.
- **`pila-bar`, a bar of Pila's own:** on every monitor, the workspaces (a click or the wheel
  switches) and a clock in your language (`--bottom`, `--clock FORMAT`). It's the default
  `autostart`. Written with iced like `pila-launcher`, on standard protocols (the layer shell and
  ext-workspace), so it also runs on other compositors; the packages include it.
- **Title bars' text a size larger:** 14 logical pixels instead of 13, the size of
  `pila-launcher`'s and `pila-bar`'s text, in the same sans-serif.
- **No terminal at login:** with no `autostart`, Pila used to open a terminal; it now starts
  `pila-bar` (Mod+Return opens the terminal). Put your terminal in `autostart` to have it back.
  With no `launcher`, Mod+D and tapping Mod now open `pila-launcher`; `launcher = ""` and
  `autostart = []` turn them off.
- **The defaults live in `config.example.toml`:** every default is written there, active rather
  than commented out, and nowhere in Pila's code. Pila has the file built in and takes whatever
  your configuration leaves out from it, so a file with only the lines you changed keeps
  working. The configuration Pila writes on the first run is that file, so it shows every
  default as a value to edit. Such a file keeps the values it was written with: a later
  release's new defaults reach the settings it doesn't have, not the ones it does.
- **`default-bindings = false`** leaves out every default binding and gesture, so only the
  config's own are left: for a scheme of one's own, or Pila as a login screen's compositor
  (greetd), where Mod+Return would otherwise open a terminal before anyone logs in. Switching
  VTs stays. Asked for in discussion #6.
- **The cursor's theme and size in the config:** `theme` and `size` under `[cursor]`, before
  `XCURSOR_THEME` and `XCURSOR_SIZE`. They apply when the file is saved, and the apps Pila
  starts get them too, so those that draw their own cursor (X11 apps) match. Asked for in
  discussion #7.
- **X11 launchers like dmenu get the keyboard:** an override-redirect window with no menu-like
  window type (dmenu, which grabs the keyboard in X) now takes the keyboard when it opens, even
  from a Wayland window, and gives it back when it closes; before, X never saw the keys unless an
  X11 window had the focus. Menus, tooltips and the like still leave the keyboard where it is.
  Reported in issue #5.
- **Removed: the shell-specific parts.** `scripts/shells`, the manual's setup guides for
  particular desktop shells, and the screenshot and demo video (with `scripts/screenshot` and
  `scripts/demo-video`) are gone; the manual has a generic section on running a full desktop
  shell instead. `scripts/check`'s layer-rule scenario now runs waybar.

## 0.6.1 (2026-11-17)

- **Less latency on a TTY:** while the screen is steady, each frame is drawn as close to the
  monitor's refresh as it safely can be, rather than right after the last one, so what apps draw
  and where the pointer goes in the meantime still make that refresh. How close follows how long
  the last few seconds of frames took, CPU and GPU, and is never under half a refresh; a frame
  that comes late anyway makes Pila draw at once on that monitor for two seconds. Not during
  Pila's animations, nor for a moment after a window or panel opens or a window changes size (its
  app is busy on the GPU then), nor while a game tears or with adaptive sync. On an AMD
  laptop with an external monitor, from a redraw asked for to the refresh that showed it: 25.8 to
  21.3 ms on the laptop's screen, 28.5 to 21.7 ms on the monitor, no refresh missed.
  `render-late = false` turns it off.
- **The frame statistics time the GPU too** (`RUST_LOG=pila::frames=debug`): each frame's GPU
  work, from its submission to its fence signaling, without waiting for it (`gpu_avg_ms`,
  `gpu_max_ms`; every frame's at trace level), and on a TTY the frames' latency, how many were
  drawn late, how far before the refresh and how long before it they were ready; `pila-check`
  reports them per monitor (`PILA_CHECK_RENDER_LATE=false` compares a run without it). Its count
  of frames no longer counts them twice once a log has the GPU's, and `pila-check --nested` no
  longer counts a refresh as missed when the host it runs in renders late.
- **Smithay moved forward** (1 commit, to 33323a9; Pila's fork rebased on it, the revision 0.6.0
  pins kept by its tag `rev-12a26c10`): apps from before wl_keyboard version 7, Xwayland among
  them, get a keymap that ends in a NUL byte, as they read it as a C string; without it they
  could read past its end.
- **Window events say where windows are:** `pila msg subscribe` windows events now carry `x`,
  `y`, `width` and `height`, as `pila msg windows` does, so a taskbar can order windows by
  position without asking again. A window being dragged or resized with the pointer keeps the
  place it had until it's let go: a drag sends one event, not one a frame.
- **`pila msg minimize <id>`** minimizes a window by its id, as a dock does through
  wlr-foreign-toplevel-management; `pila msg focus <id>` brings it back. Refused while the
  session is locked, like `focus` and `close`.
- **Window rules can choose the monitor and keep the focus where it is:** `open-on-output` opens
  a window on a monitor named by connector (`eDP-1`) or as an `[[output]]` section does
  (`"Make Model"`), falling back to the pointer's when that one is unplugged or off;
  `open-focused = false` opens it on top while the keyboard stays with the window you were typing
  in. Pila run nested now has the app id `pila`, so a rule can send a development run to another
  monitor, out of the way.
- **A blurred window's backdrop is kept while it's hidden:** on another workspace or minimized,
  it was dropped and made anew when the window showed again, textures and all, which on battery
  cost that frame its refresh; it now stays until the window closes.
- **Blur doesn't allocate on every frame of a resize:** a blurred panel animating its size or a
  blurred window being resized kept getting new textures for its backdrop, one set per frame;
  they're now a little bigger than needed (with room to grow, once one has grown out of
  them) and kept while it grows or shrinks, and given back once it shrinks to under half of
  them. A shell's control center opening went from 6 allocations to 3.

## 0.6.0 (2026-11-16)

- **Tearing for games** (wp-tearing-control): a fullscreen game with vsync off (Vulkan's
  immediate present mode, which Mesa turns into the request) has its frames shown at once with an
  asynchronous page flip, on a TTY, when the driver takes it. `tearing = false` in the
  configuration turns it off. The frame statistics count those frames apart (`torn`), and
  `pila-check` runs `vkcube` that way where it's installed.
- **Xwayland starts when an X11 app needs it, and can stop when none is left:** Pila reserves the X
  display at startup (`DISPLAY` is set for the session at once, and the startup programs no longer
  wait for X) and starts Xwayland when the first X11 app connects, so a session with no X11 app runs
  no X server. With `[xwayland] idle-timeout = <seconds>` it also stops once no X client is left (a
  background X program keeps it running), and the next X11 app starts it again. Xwayland that dies
  or keeps failing is started again by the next app rather than right away. It now gets Pila's whole
  environment (Smithay's `spawn_with_inherited_env`), so variables such as `MESA_*` or
  `__GLX_VENDOR_LIBRARY_NAME` reach it.
- **X11 apps can be turned off, and Xwayland is optional:** `[xwayland] enabled = false` reserves no
  X display and sets no `DISPLAY`, for a session with no X11 at all; turned off while X11 apps run,
  Xwayland stays for them until it stops. Applies on save, like the rest.
- **X11 apps that grab the keyboard get Pila's shortcuts too** (xwayland-keyboard-grab): a
  virtual machine or remote desktop running through Xwayland gets every key while it has the
  focus, as Wayland ones already did (keyboard-shortcuts-inhibit); clicking another window gets
  out, and the binding that quits Pila still works.
- **Apps can fade a surface without drawing it again** (wp-alpha-modifier): video players and
  overlays set its transparency, and Pila applies it on screen, in screenshots and screen
  sharing, and behind blur (a window faded that way gets its backdrop).
- **Apps can move the pointer within their own window** (wp-pointer-warp), as games and SDL 3
  do to recenter it: only the app the pointer is on, only onto its own surface, never while the
  screen is locked.
- **Blur behind popups:** a menu or an input method's candidate window that asks for blur
  (ext-background-effect, as fcitx5's does when its theme has blur) gets it, over windows and
  panels alike; only windows and panels got it before. A rule's `blur = false` refuses it to their
  popups too.
- **Snapping to the top and bottom halves:** the actions `snap-top` and `snap-bottom` (no default
  binding: `Mod+Up` still maximizes and `Mod+Down` restores; bind them to the arrows in
  `[bindings]` if you'd rather have halves). Snapping the same side again restores the window, as
  with the other snaps.
- **The manual says which desktop tools work on Pila** ("Building your desktop"): those tried on it,
  those that work in part (ironbar, nwg-panel: no workspaces), those that can't work and why (bars
  that talk to another compositor's own IPC, X11 bars), and a waybar setup with Pila's workspaces
  (`ext/workspaces`) and taskbar (`wlr/taskbar`).
- **Resizing a window with the pointer keeps up with the app:** Pila sent a new size for every
  pointer motion (up to a thousand a second with a fast mouse), more than an app draws; it now
  sends the next one once the app has acknowledged the last, so a slow app (a browser, an
  Electron app) gets the latest size instead of a backlog of old ones.
- **An app that destroys its xdg-decoration object draws its own decorations for good:** Pila's
  title bar went, but came back when the window was mapped again.
- **Input methods: candidates can be clicked in a popover's text field** (a GTK search field):
  the candidate window got no pointer while the popover was open, and a click on it closed the
  popover. It dates from 0.5.8, when input methods came.
- **For packagers: Xwayland is now an optional dependency** (`optdepends` in `pila-git`, a
  recommendation in the `.deb` and `.rpm`, and `withXwayland ? true` in the Nix package): Pila
  runs without it, with no X11 apps.
- **For packagers: Pila's Smithay fork also carries Smithay's X display reservation** (its pull
  request #2269, not merged yet), with which Xwayland starts on demand and its window manager
  leaves no event source behind.
- **For packagers: Pila's Smithay fork also carries Smithay's asynchronous page flips** (its pull
  request #1948, not merged yet), still at `github.com/igalmarino/smithay`, branch `pila`, now on
  Smithay's main of 2026-11-14 (xdg-decoration version 2). The revisions earlier releases pin
  are kept by tags (`rev-b8f524b3`, `rev-64a43022`).

## 0.5.9 (2026-11-15)

- **An X11 app's own transparency shows with rounded corners:** an app that sets
  `_NET_WM_WINDOW_OPACITY` on its window (terminals' opacity settings, `picom-trans`) was drawn
  opaque when the window had a `corner-radius`. It dates from 0.2.0, when rounded corners came.
- **Modal X11 dialogs keep the focus,** as Wayland ones (xdg-dialog) already did: a dialog that
  sets `_NET_WM_STATE_MODAL` (before it shows, or later) gets the focus whenever its parent window
  would, so a click on the parent or Mod+Tab to it lands on the dialog.
- **Monitors placed automatically next to one with a fractional scale no longer overlap it:** the
  next monitor started one column of pixels too far left (a 1280 px wide monitor at 1.5 is 853.33
  px wide; Pila placed the next one at 853 where its layout ends at 854).
- **The headless backend picks a mode as bare metal does:** of several modes of the same size it
  took the fastest instead of the preferred one.

## 0.5.8 (2026-11-15)

- **Input methods (IME):** fcitx5 and other input methods that speak input-method-v2 type Chinese,
  Japanese, Korean and the like into apps that speak text-input-v3 (GTK, Qt 6.7 and later, foot,
  kitty, Chromium and Electron with `--enable-wayland-ime`, and launchers), with the candidate window
  below the text cursor (above it at the bottom of the screen). Pila's keybindings come before the
  input method, menus open while it holds the keyboard, and it never sees what's typed on the lock
  screen. The manual says how to start it ("Input methods"). Asked for in discussion #2.
- **For packagers: Smithay comes from Pila's fork for now,** `github.com/igalmarino/smithay`,
  branch `pila` (the commit is in `Cargo.lock`): Smithay's own main branch plus one fix, kept until
  Smithay has it. Smithay had started dropping a text cursor with no width, which GTK sends, so
  candidate windows sat at the corner of GTK apps' windows.

## 0.5.7 (2026-11-14)

- **GTK 4 apps no longer freeze after a dropdown or menu is opened a second time:** GTK hides a
  popover by destroying its popup and opens the next one on the same surface, which Pila took
  for one being hidden: it never told the new popup it could show itself, so the app drew
  nothing, and its grab kept every click from the app (the button stayed pressed) until a key
  was pressed. Reported by a tester in HandBrake and a text editor; it dates from 0.4.2.
- **A key binding can repeat while its key is held:** written as a table,
  `"XF86AudioRaiseVolume" = { action = "spawn wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+", repeat = true }`,
  it runs again after `repeat-delay` and at `repeat-rate` (`[keyboard]`) until the key is
  released or another one is pressed: for volume and brightness keys. The plain form is
  unchanged and runs once a press. Only keys repeat, not a wheel notch or a modifier alone.
- **Num Lock on at startup:** `numlock = true` under `[keyboard]`. Pila also turns it on again when
  a change to `[keyboard]` loads another keymap, which starts with every lock off.
- **A quick turn of the mouse wheel switches one workspace, not all of them:** Mod and the wheel
  ran the binding for every notch, and a light flick is many notches. A wheel binding now doesn't
  run again within `wheel-binding-cooldown` (`[input]`, 150 ms by default; 0 runs it each
  notch) of its last run; the notches in between are dropped, and don't reach the app either.
- **An `[[output]]` section for a monitor that already has one is a warning:** only the first
  one counted, and the second's settings did nothing, silently. `pila --check-config` prints the
  warning after OK, and a notification says it at startup and on reload.

## 0.5.6 (2026-11-14)

- **`corner-radius` no longer gives GTK apps a double corner:** apps that draw their own shadow
  draw their own rounded corners too, and Pila rounded the window again, inside the app's shadow,
  which kept its own corner. The global `corner-radius` now leaves them as they are; a window
  rule's `corner-radius` still rounds them. Reported by a tester.
- **A dock or panel menu closed by a click on the bare desktop gives the keyboard back to a
  window:** the menu's keyboard grab outlived it, and the next key gave the keyboard to the
  panel, which takes none, so typing went nowhere. Found by the input invariants the tests now
  check.
- **A window unmaximized and then made fullscreen quickly comes back to its own size:** if the app
  hadn't drawn its unmaximized size yet when it went fullscreen, Pila took the size it still had
  drawn, the maximized one, for the one to come back to. It now keeps the size it asked the app
  for. Found by running the test suite repeatedly under load.
- **A snapped window dragged out of its half stays under the pointer:** since 0.5.4 it came back
  to its size only once the pointer had moved, and it lost that first stretch of the drag: it
  ended up a little above the pointer with Pila's title bar, and well above it when dragged by an
  app's own title bar (GTK's starts its drag late).
- **A taskbar's "unmaximize" or "leave fullscreen" on a window that isn't either no longer moves
  it:** like GTK 4's request fixed in 0.5.5, it took the window to its default place, two thirds of
  the screen, centered. Requests from taskbars and docks (wlr-foreign-toplevel) for the state a
  window is already in now change nothing, as the app's own ones do.

## 0.5.5 (2026-11-14)

- **A GTK 4 window dragged by its own title bar no longer jumps:** GTK asks to leave fullscreen
  as such a drag starts, even when the window isn't fullscreen, and Pila took that for a window
  coming back from fullscreen: it moved it to its default place, two thirds of the screen,
  centered. A request for the state a window is already in now changes nothing (Pila still
  answers it, as the protocol says).
- **Pila writes your configuration file the first time:** with no `~/.config/pila/config.toml`
  (or `$XDG_CONFIG_HOME/pila/config.toml`), it creates it from `config.example.toml`, every line
  commented out, so nothing changes until you edit it, and changes apply as you save from the
  first session on. It never replaces a file you have, nor a symlink of a dotfiles manager, and
  `--config` paths are left alone.
- **The click after a dock or panel menu isn't lost anymore:** once a menu went away, Pila ended
  its grab only at the next input event and spent that event on it, so the pointer stayed where
  the menu was and the next click landed there (in a shell's dock, the right click after
  "Minimise" opened nothing). The keyboard also goes back to a window when the menu belonged to a
  panel that takes none.
- **A minimized window keeps its icon in per-monitor taskbars:** Pila told
  them it had left its monitor, and so did a window on another workspace. Both now stay on the
  monitor they come back to.

## 0.5.4 (2026-11-14)

- **Mod and the mouse wheel switch workspaces:** a notch down goes to the next workspace, up to
  the previous one, and with Shift too the focused window goes there (as `Mod+Page_Down` and
  `Mod+Page_Up`). Any binding can now use a wheel notch as its key: `WheelScrollUp`,
  `WheelScrollDown`, `WheelScrollLeft`, `WheelScrollRight`, e.g.
  `"Mod+WheelScrollRight" = "focus-output next"`; `"Mod+WheelScrollDown" = "none"` gives the defaults
  back to the apps.
- **A click on the title bar of a maximized, fullscreen or snapped window leaves it as it is:**
  the press alone brought it back to its old size, as dragging it does. Now the pointer has to
  move a few pixels first; the same goes for a Mod+click anywhere in the window.

## 0.5.3 (2026-11-13)

- **A fullscreen app no longer stalls Pila as it starts or stops being shown as it is** (direct
  scanout) on AMD laptops with a second monitor: Pila's own frames were ARGB8888 and an app's buffer
  went to the screen as XRGB8888, and amdgpu answers that change of format by waiting for every
  monitor's next refresh, up to 14 ms with nothing else handled. Pila now draws its frames as
  XRGB8888 too.
- **For testers: `pila-check` works on more systems:** it quits by itself when Pila runs under
  another name (Nix's package runs it as `.pila-wrapped`, so it waited to be quit by hand), and
  its capture stream works with a Python that lacks `os.memfd_create`. Both were found on an
  NVIDIA GTX 1060 under NixOS, where everything else passed.

## 0.5.2 (2026-11-12)

- **For testers: `pila-check` comes with Pila:** the packages, Nix's build and `scripts/install`
  install `scripts/check` as `pila-check` (its helpers in `share/pila/check`), so a test run on
  your hardware needs no checkout: run `pila-check` from a TTY, and its log is in
  `~/.local/state/pila`. `pila-check --help` says how to use it.
- **A logo:** three stacked windows in Pila's default title-bar colors, at the top of the README
  and the manual (`docs/logo/`). It's also the session's icon: `scripts/install` and the
  packages install it as `pila` in the hicolor theme, and the session entry names it
  (`Icon=pila`), for login screens that show one.
- **Less CPU while you use the mouse, resize or switch windows:** a high-rate mouse (1000 Hz)
  made Pila redo its housekeeping (taskbars, IPC events, idle timers) for every event; it now
  does it once per loop iteration, or when the pointer changes focus or monitor. Resizing a
  window no longer redraws its title's text in every frame, and switching focus back and forth
  (Mod+Tab) draws no title bar again. Touchpad gestures an app handles itself (pinch to zoom) and
  apps setting the cursor they already have no longer redraw the screen, and blurred or animating
  windows are drawn only for the monitors they reach. `RUST_LOG=pila::frames=debug` also reports
  the event loop's own work (`loop`).
- **The first window opened is no slower to draw than the next ones:** the GPU driver's one-time
  setup for drawing a window (rounded corners, title bar textures) happened in its first frame,
  which took more than twice as long as later ones (2.6 ms against 1.3 ms on an AMD laptop); Pila
  now does it at startup. `pila::frames` also logs, when a monitor starts or stops showing an
  app's buffer as it is (direct scanout), how many other monitors had a frame on its way.
- **A binding with an unknown action names its keys as written** (`binding "Mod+X": unknown
  action ...`), not as Pila had parsed them; gestures too.
- **How Pila is made:** a page on how the project uses AI assistance, who decides what, how
  changes are verified and what has gone wrong so far (`docs/HOW-PILA-IS-MADE.md`, linked from
  the README's "Use of AI").
- **The manual says how to change how Pila looks:** colors, rounded corners, shadows,
  animations, blur and opacity, in a section of their own ("Looks").

## 0.5.1 (2026-11-11)

- **Shake the mouse to find the pointer:** as on KDE Plasma, shaking the pointer quickly back and
  forth makes the cursor grow, up to three times its size, while you shake it; it shrinks back a
  moment after you stop. Moving it along a line or round a circle doesn't count. It's on by
  default: `[cursor]` sets how big it gets (`shake-magnification`, 1.5 to 5) or turns it off
  (`shake-to-find = false`). Named cursors are drawn from the theme's larger images, so they stay
  sharp; on the lock screen too.
- **A workspace switch no longer starts a frame late:** its slide made textures as large as each
  monitor as it began, which cost the graphics card several milliseconds and, on a TTY, could make
  the slide's first frame miss a refresh. They're now made ahead and kept (two per monitor, some
  tens of megabytes of graphics memory; none with the slide turned off).
- **Fullscreen videos and games skip Pila's drawing:** on a TTY, a window that is all a monitor
  shows (fullscreen, with nothing over it) now goes to the screen as its app drew it, with no
  frame of Pila's in between: less work for the GPU, and battery (direct scanout, on the main
  GPU's monitors, when the driver takes the app's picture). If such a window flickers or shows
  wrong colors, `direct-scanout = false` in the configuration turns it off; please report it.
- **A maximized or fullscreen window leaves the monitor next to it alone:** each of its frames
  made the neighboring monitor look for something to redraw (room was kept for a shadow such a
  window never has), and a frame of that monitor's own could then show a refresh late.
- **Monitors on another GPU, to try:** on a laptop with two GPUs, the HDMI or DisplayPort output
  is often wired to the dedicated one, and a dock may have its own; Pila shows nothing there.
  With `other-gpus = true` in the configuration it draws them with the main GPU and copies the
  picture to theirs, GPUs plugged in and out included. Off by default: it's new and not yet tried
  on such a machine, so reports are very welcome. `pila-report` lists the GPUs in use and says
  which monitors are on one that isn't.
- **X11 windows show up at once:** an X11 window could open blank (and the window focused
  before kept its focused title bar) until something else redrew the screen, when Xwayland
  told Pila which surface was the window's after drawing it.
- **Smithay moved forward** (10 commits, to 3a08ff9): an X11 app that withdraws its window the
  ICCCM way (a synthetic `UnmapNotify`) now has it unmapped, an endless loop of unmaps when an X11
  window was reparented is gone, and damage with a negative size from an app, which crashed a
  debug build of Pila, is dropped.

## 0.5.0 (2026-11-10)

- **Tap Mod to open the launcher:** pressing and releasing Mod alone (Super, by default on a TTY)
  opens the launcher, as Mod+D does, the way other desktops open their menu. It runs only if no
  other key, click, scroll or gesture came in between, so every shortcut with Mod stays as it
  was. `"Mod" = "none"` in `[bindings]` turns it off, and `"Mod"`, `"Super"`, `"Alt"` or `"Ctrl"`
  alone can be bound to any action.
- **Alt+Tab switches windows:** as on other desktops, `Alt+Tab` and `Alt+Shift+Tab` now do what
  `Mod+Tab` and `Mod+Shift+Tab` do, whatever the Mod key is (Super, by default on a TTY). Apps no
  longer get Alt+Tab; `"Alt+Tab" = "none"` in `[bindings]` gives it back to them.
- **The focus goes back to the window you were using:** closing a window, a launcher going away
  or unlocking the screen gave the keyboard to the topmost window, which with a window kept on
  top (`toggle-always-on-top`) was always that one. It now goes to the window used most recently.
- **Room for busy sessions:** Pila raises its limit of open files (usually 1024) to the most the
  system allows, since it holds some for every app, buffer and frame in flight; a session with
  many windows could run out and drop apps. Programs it starts still get the usual limit.
- **Screenshots of a region past the limits fail instead of crashing:** a `wlr-screencopy` region
  with a negative size or a place past 32768 px could crash a debug build of Pila; the frame now
  fails, as other numbers from apps past the limits do since 0.4.2.
- **Nothing comes back behind the lock screen:** an app holding an activation token from before
  the screen locked could bring a minimized window back while it was locked (out of sight; the
  keyboard stayed with the lock screen). It's now ignored until unlocked.
- **`pila msg subscribe` hears the monitors go off:** with every monitor off nothing is drawn,
  and the `outputs` events waited for a frame, so a subscriber never saw `power: false`.
- **`pila msg outputs` shows monitors that are off:** marked `[off]`, as 0.4.2 said it did
  (only `--json` had their `power`).
- **Nothing reaches an app behind the lock screen, by construction:** while the screen is
  locked Pila refuses keyboard and pointer input (focus, keys, clicks, scrolls, gestures, drags)
  to anything but the lock screen where input reaches apps, whatever path sends it, instead of
  relying on each path to check. It found one: locking sent the window under the pointer one
  last pointer motion, after the lock had begun.
- **No crash for an X11 window without a surface given the pointer:** a popup grab could hand
  the pointer to an X11 window Xwayland hadn't given a surface yet, which crashed Pila; such a
  window now gets nothing.
- **A swipe begun as the screen locks runs nothing:** its action (show the desktop, say) ran
  behind the lock screen if the screen locked during the swipe.
- **Less CPU with two monitors:** moving the mouse on one monitor, or an app drawing on it,
  redrew the others for nothing (measured: 1000 to 5000 times a second, about 5% of a core). Each
  monitor is now redrawn only for what shows on it, and no longer builds what it would draw of
  the windows on the others (measured headless, six windows on one monitor: a redraw of the
  other a third cheaper). On a TTY, a redraw that may have nothing to show (an app sending the
  same picture again) waits for the next refresh, at most one per refresh. The frame statistics
  (`RUST_LOG=pila::frames=debug`) count a frame shown a refresh later than it could have been as
  missed (`late`).
- **Less CPU with monitors of different scales:** drawing one monitor re-rasterized the title
  bars of the windows on another monitor of another scale (the title text and the buttons), and
  the next time that one was drawn, again. Each scale now keeps its own.
- **Window titles appear as soon as the fonts load:** a window that opened before then showed
  its title only at the next redraw of its monitor.
- **No gamma warning at every login:** the log warned that setting gamma held Pila past a
  frame's budget each time it started, for the expected cost of setting up the monitors. The
  warning is now kept for a night light's changes, where it means something.
- **Crashes say where they happened:** the log has a backtrace after "pila crashed" without
  setting `RUST_BACKTRACE`, so a crash in a login session can be reported as it is, without
  reproducing it. The AUR package is no longer stripped, so the backtrace has the names of the
  functions (about 9 MB more).
- **The AUR package says when Rust is too old:** with a Rust older than 1.89 it stops with "pila
  needs Rust 1.89 or newer" instead of Cargo's error about `edition2024`, which didn't say what to
  do.
- **For testers: `scripts/check` no longer hangs with fewer than three workspaces:** it took the
  number from your config and stopped at a switch to workspace 3, leaving Pila open. It now sets
  its own, and a step that fails ends the run, with the step's line in the result.
- **For testers: `pila --version` says `-dirty` for a build of uncommitted changes:** it gave
  the last build's version instead, so a run with a fix looked like the commit before it.
- **For testers: redraws that find nothing new are counted:** the frame statistics have them per
  output apart from frames (`idle`, with their CPU time), and `scripts/check` reports them.

## 0.4.2 (2026-11-09)

- **Apps can no longer crash Pila with absurd sizes:** a window made wider than any screen (a
  subsurface far out stretches it) crashed Pila drawing its title bar, and other sizes and
  positions near the limits of a 32-bit number overflowed in layout and drawing. Sizes and places
  a client gives its surfaces (buffers, viewports, subsurfaces, window geometry, layer surfaces,
  popups) past 32768 px are now a protocol error that ends that client, and the log says which.
- **Monitor positions within ±1000000:** a display tool (`wlr-randr`, kanshi) placing a monitor
  near the limits of a 32-bit number could crash Pila; now the change fails and nothing moves.
  `[[output]]` positions past it are a config error naming the monitor.
- **Output configuration follows the protocol more closely:** setting a monitor's property twice
  in one change is the protocol error wlroots gives (`already_set`), instead of the second value
  winning.
- **Events from `pila msg`:** `pila msg subscribe` (or a `subscribe` request on the socket) sends
  the windows, workspaces and monitors when they change, whole lists each time, so a bar or a
  script follows them without asking over and over (docs/IPC.md).
- **Keyboard layouts from a bar or a binding:** `pila msg keyboard` lists the layouts of
  `[keyboard]` and says which is in use, `pila msg subscribe keyboard` follows it, and the action
  `switch-layout <n|next|prev>` changes it, from a key binding or `pila msg action`.
- **`PILA_SOCKET` for shells started as services:** a login session exports it to the D-Bus
  session bus and systemd's user manager with `WAYLAND_DISPLAY`, so a shell started by systemd
  finds `pila msg`'s socket.
- **Turning the monitors off:** the actions `power-off-monitors` (until any input; the key or
  click that wakes them does nothing else) and `power-on-monitors`, for a binding or a shell's idle
  settings through `pila msg action`, which allows them with the screen locked (as it does
  `switch-layout`, for a lock screen's password). `pila msg outputs`
  tells each monitor's `power`.
- **Manual: desktop shells on Pila's terms:** their idle monitor off and Log out through
  `pila msg`, and a waybar module that follows the keyboard layout.

## 0.4.1 (2026-11-08)

- **Sharing a single window keeps going when it's hidden:** a shared window that another window
  covered dropped to one frame a second, and one minimized or on another workspace froze, as
  browsers and video players stop drawing when nothing asks for a frame. It now draws at the
  share's pace. The manual says how to share single windows ("Screenshots and screen sharing").
- **Night light without stutter:** each change of a night light (every step of its fade) held
  Pila for a whole refresh, 16.7 ms at 60 Hz, while input, apps and every monitor waited. Changes
  now go out without waiting (about 2 ms on an AMD laptop); drivers without `GAMMA_LUT` keep the
  old way.
- **Night light keeps working after a VT switch:** a change made while another VT was in front
  failed, and a shell's night light stopped until it was restarted. Changes made meanwhile now
  apply when you come back.
- **The session starts even when Xwayland can't:** Xwayland dying at startup (a crash, a broken
  install) left Pila waiting for it forever, busy, without starting your shell or terminal. Now
  only X11 apps are missing.
- **A failing monitor no longer freezes the others:** when one monitor's frames fail (not enough
  bandwidth, a dock dropping out), it retries on its own, waiting longer each time and logging
  ever more rarely, while the others go on drawing.
- **A monitor whose mode doesn't fit comes on in a lighter one:** a lower refresh rate, then
  smaller sizes (two 4K monitors on one dock, say), instead of staying off; the log says which.
- **Monitors whose DisplayPort link fails are set up again:** after a resume or through a dock,
  a monitor the kernel marks as having a bad link (a black screen) gets its mode set again.
- **Screen capture fixes:** a capture whose copy failed waited forever on a still screen; a
  shared window changed resolution when it left the screen; a dma-buf the GPU can't draw into
  failed with an unknown error instead of asking for another buffer; a copy still on the GPU
  wrote into a buffer its client had already taken back. Captured frames now carry the time
  their content shows on screen, which OBS uses to keep sound and picture together.
- **Screen sharing costs less:** sharing a monitor reuses the frame just drawn for it instead of
  drawing everything again (on a TTY, unrotated monitors), and screenshots and shares reuse their
  textures. Captures are held to the same frame budget as the monitors.
- **Adaptive sync:** apps are told the refresh is variable when it is (video players and games
  pace their frames by it), and adaptive sync stays on when a monitor wakes from blanking
  (`wlopm`, idle): it used to stay off until the next config reload.
- **Monitors on a second GPU** don't work yet, and now the README says so: on many laptops with
  two GPUs, NVIDIA ones especially, the HDMI and DisplayPort outputs are on the dedicated one.
  `pila-report` marks those monitors.
- **`pila-report` for a user named like a system directory:** for a user called `dev`, paths
  such as `/dev/dri/card1` were hidden as if they held the user name.
- **Pila is written "Pila"** in the docs, the man page and the login screen's session menu; the
  command, packages and paths stay `pila`.
- **For testers:** `scripts/check` also streams a screen capture, checks adaptive sync with
  `--vrr`, holds gamma changes and captures to the frame budget, and says so when given an
  option it doesn't know. Pila's display code also runs on GitHub on the kernel's virtual
  display driver (`vkms.yml`), with a monitor plugged in and out.

## 0.4.0 (2026-11-08)

- **Smithay's main branch:** pila now builds against Smithay's development branch (a git
  dependency pinned to one commit) instead of the 0.7.0 release
  from June 2025, so fixes reach pila without waiting for a release. Building pila needs
  network access to GitHub for it (the AUR package and Nix flake fetch it by its commit).
- **Quickshell shells no longer crash:** a shell that hid a panel or
  launcher, or unlocked the screen, by destroying the surface's role and then committing it
  with no buffer was disconnected by a protocol error. One lost its connection each time its
  launcher closed.
- **Virtual keyboards** (`wtype`, on-screen keyboards, a clipboard history pasting) are
  keyboards now: their keys can use pila's bindings and count as activity (the screen doesn't
  blank while you type on one), and they type with their own keymap. Your keymap, layout and
  Caps/Num Lock come back with the next key you press or when the tool exits.
- **Window menu:** right-clicking a title bar, pila's or an app's own (GTK, browsers), opens a
  menu to minimize, maximize, keep the window on top or on all workspaces, or close it.
- **Windows asking for attention** (a chat message, an X11 app's urgency hint) get an amber title
  bar (`[colors] bar-urgent`) and mark their workspace in bars that show workspaces, until you
  focus them.
- **Window rules follow title changes:** a rule on a title applies when a window takes that title
  later (a browser's page) and stops when it changes again; what applies when a window opens
  still only applies then.
- **Clicking a notification opens its app in front:** a notification or panel (which takes no
  keyboard) may pass the click on to the app it opens, which then gets the focus.
- **Launchers and dialogs above the windows get the keyboard when they appear** if they ask
  for it on demand (a shell's welcome dialog, some launchers), instead of after a click.
- **Shells started by a pila inside another session find pila:** programs pila starts no
  longer inherit another compositor's sockets (`SWAYSOCK`, `HYPRLAND_INSTANCE_SIGNATURE`, …),
  so a shell in a pila started from Sway, Hyprland or another compositor doesn't drive that
  compositor instead.
- **Why a client went away is in the log:** a program pila disconnects for a protocol error is
  logged as a warning with its process id and the error.
- **Desktop shells checked feature by feature:** two Quickshell shells ran on a headless pila
  through everything of theirs that depends on the compositor (bar, launcher, workspaces,
  taskbar, notifications, clipboard, screenshots, night light, display settings and power, lock
  screen). Both work; the manual says what they can't do until they know pila by name
  ("Building your desktop").
- **Dragging shows what you drag:** the icon an app gives a drag (a file, a tab) follows the
  pointer until the drop; pila never drew it before.
- **Panels hidden and shown again come back:** a layer surface (panel, launcher, dock) hidden
  by committing no buffer is shown again by its next initial commit, as the protocol says.
- **A window mapped again keeps its decorations:** a client that hid a window and showed it
  again lost the title bar.
- **X11 apps can start minimized** (they ask with `WM_HINTS`), instead of opening and taking
  the focus.
- **X11 apps with client-side shadows** (`_GTK_FRAME_EXTENTS`, GTK 3) are measured without the
  shadows.

## 0.3.1 (2026-11-07)

- **Config errors say what's wrong:** a notification about an invalid config used to show only
  the start of the parser's message ("TOML parse error at line 127, column 4 |") and cut off the
  line that said what the problem was. Now it reads `line 127, column 4: <what's wrong>`, and a key
  binding outside `[bindings]` says that bindings go there.
- **`config.example.toml`:** every value is commented out, so a copy of it changes nothing until
  you edit it, and each one says whether it's the default or an example. Table headers stay
  active, so a line you uncomment lands in its table: the list of default bindings used to sit
  above `[bindings]`, and uncommenting one put it in `[input.mouse]`. Also explained now: the
  keyboard `model`, `[input.mouse]`, the range of `scale` and Mod+drag.
- **Packages:**
  - `pila-git` no longer runs the test suite while building it: the tests need a working EGL
    setup, which a virtual machine may not have, and CI already runs them on every commit.
  - `bash` is a dependency (`pila-session` and `pila-report` are bash scripts).
  - `wl-clipboard` is recommended: without `wl-copy`, Mod+Shift+S did nothing, with no message.
  - `swaylock` and `pciutils` are suggested, not recommended, in the `.deb` and `.rpm`, so apt
    and dnf don't install them by default: a desktop shell may have its own lock screen,
    and only `pila-report` uses `lspci`. They're optional in `pila-git`.
- **`pila-report`** says whether `wl-copy` is installed, and hides your user name only as a whole
  word: a name that ended another word ("destroy") mangled the log and gave the name away.

Since 0.3.0, a tester has run pila from LightDM in an Arch virtual machine.

## 0.3.0 (2026-11-06)

- **Removed:** `disable-internal-display`. **A config that has it is now invalid:** pila says so
  and starts with the defaults, so delete the line. To turn a built-in screen off, use
  `enabled = false` in its `[[output]]`; it comes back if no other output is left on.
- **Shadows:** a shadow under each window, stronger for the focused one, on by default.
  `[shadow]` sets its size, offset and colors or turns it off, and a window rule's `shadow` does
  it per app. Maximized and fullscreen windows have none, nor do those that draw their own.
- **Colors:** `[colors]` sets the title bars, buttons, the background and the snap preview,
  applied when the config is saved.
- **Windows:**
  - Mod+T keeps a window above the others, also over a fullscreen one, and Mod+P keeps it on all
    workspaces. Rules can do it when a window opens: `always-on-top`, `all-workspaces`.
  - Dragging to a corner snaps to a quarter of the screen. `snap-top-left` and the like are
    actions too, with no default binding.
  - Mod+H shows the desktop; again, it brings the windows back.
- **Monitors:** Mod+Shift+O sends a window to the next monitor, keeping its place, or filling it or
  snapping to the same side. Mod+O moves the pointer and focus there.
- **`pila msg`:** lists windows, monitors and workspaces as text or JSON and runs any action, for
  scripts. Actions are refused while the session is locked. The format is in `docs/IPC.md`.
- **New default bindings:** Mod+T, Mod+P, Mod+H, Mod+O and Mod+Shift+O. Your `[bindings]` still win.
- **Fixes:** `scripts/check` no longer counts as missed refreshes the time a monitor rests while
  another one animates.

All of this was tried on one AMD laptop with an external monitor; other GPUs are untried.

## 0.2.3 (2026-11-06)

- **Blur behind windows:** a translucent window can show what's behind it blurred, when the app
  asks for it through ext-background-effect or a `[[window-rule]]` sets `blur = true`;
  `blur = false` refuses an app's request. The blur keeps the window's rounded corners, follows
  its animations and workspace switches, and appears in captures of the screen but not of the
  window alone. Measured on one AMD laptop (no refresh missed); other GPUs are untried.
- **Opening links and files:** programs such as kitty do it with `xdg-open`. Without it a click on
  a link does nothing and leaves no error anywhere, so `xdg-utils` is now a recommended package and
  `pila-report` says whether `xdg-open` is installed.
- **Notifications:** an invalid config is now notified at startup, not only on a reload. A problem
  stays on screen for 10 s, not 3. `libnotify` is a recommended package, and the log says once if
  `notify-send` is missing.
- **Session log:** written as it happens, so `tail -f` follows it and the end of a run isn't lost.
- **`pila-report`** says which GPU pila draws with, however long the log.
- **Docs:** a rule's `opacity` can only lower what a window draws: `opacity = 1` won't make a
  translucent terminal opaque.
- **Tests:** tests that depended on the machine's speed now hold on a slow one.

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
  `[[output]]` sections that name a monitor (`name = "GSM 27GL850"`) instead of a
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
  their own, e.g. `namespace = "my-shell-*"`) and blur.
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
