# Changelog

## Unreleased

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
- **Manual: Noctalia and Dux Shell on Pila's terms:** their idle monitor off and Log out through
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
  failed, and Noctalia's night light stopped until it was restarted. Changes made meanwhile now
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
  dependency pinned to one commit, as niri and cosmic-comp do) instead of the 0.7.0 release
  from June 2025, so fixes reach pila without waiting for a release. Building pila needs
  network access to GitHub for it (the AUR package and Nix flake fetch it by its commit).
- **Quickshell shells (Dux Shell/DMS, …) no longer crash:** a shell that hid a panel or
  launcher, or unlocked the screen, by destroying the surface's role and then committing it
  with no buffer was disconnected by a protocol error. Dux Shell lost its connection each
  time its launcher closed.
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
  for it on demand (Noctalia's welcome dialog, some launchers), instead of after a click.
- **Shells started by a pila inside another session find pila:** programs pila starts no
  longer inherit another compositor's sockets (`NIRI_SOCKET`, `SWAYSOCK`,
  `HYPRLAND_INSTANCE_SIGNATURE`, …), so Noctalia or Dux Shell in a pila started from niri,
  Sway or Hyprland don't drive that compositor instead.
- **Why a client went away is in the log:** a program pila disconnects for a protocol error is
  logged as a warning with its process id and the error.
- **Noctalia and Dux Shell checked feature by feature:** `scripts/shells` runs each on a headless
  pila and goes through everything of theirs that depends on the compositor (bar, launcher,
  workspaces, taskbar, notifications, clipboard, screenshots, night light, display settings and
  power, lock screen). Both work; the manual has a Dux Shell setup now, and says what neither
  can do until it knows pila by name ("Building your desktop").
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
    and dnf don't install them by default: a shell such as Noctalia has its own lock screen,
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
