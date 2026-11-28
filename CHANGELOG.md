# Changelog

## Unreleased

- **Changed (breaking):** Direct screen capture permissions are kept in one place, the
  configuration. The capture prompt no longer has **Remember**: Allow lasts until you log out, as
  it did without it, and `~/.local/state/pila/capture-permissions.json` is no longer read or
  written. With two stores, a rule and a remembered answer could disagree; now the `[[permission]]`
  rules are the only thing that lasts, and the prompt says so. **Migration:** for each program you
  had remembered (`pila msg capture-permissions` on 0.9.8 listed them, or see that file), add a
  rule, then delete the file:

  ```toml
  [[permission]]
  executable = "/usr/bin/wf-recorder"
  access = "allow"
  ```

  `pila msg capture-permissions` and `forget-capture-permission` work as before, for what was
  allowed in the prompt; `persistent` is now always `false`. Portal screen sharing keeps its own
  Remember (the portal's standard mechanism).
- **New:** A capture indicator. While a program captures the screen, through the portal or
  directly (`grim`, `wf-recorder`), a small accent square lights the middle of the top edge of the
  monitor being captured, on the screen only, never in the capture (`[permissions] indicator =
  false` turns it off). An icon in the system tray (waybar's, or any bar's that hosts
  StatusNotifierItems) shows it too, while something captures: its tooltip says who captures what,
  and its menu stops a capture; a program stopped can't capture again until it restarts
  (`[permissions] tray = false` turns it off). `pila msg captures` lists who captures what, and
  `pila msg --waybar captures` follows it as a waybar custom module.
- **New:** `pila msg captures` also names the capture that ended last, and how long ago (`last`,
  with `ended` in Unix seconds, in the JSON answer and the `captures` event), so a screenshot over
  before you could look at the indicator can still be told apart.
- **New:** `pila msg idle-inhibitors` lists the apps keeping the screen on now: those that asked
  over D-Bus (`org.freedesktop.ScreenSaver`), with the reason they gave, and the visible windows
  with a Wayland idle inhibitor. Handy to find a forgotten browser tab keeping the session awake.
- **New:** Apps calling `org.freedesktop.ScreenSaver`'s `SimulateUserActivity` (remote desktops,
  some players) now count as user activity, as input does, and `GetActive` answers whether the
  session is locked; both used to fail as unknown methods.
- **Changed:** The quit dialog's and the capture prompt's buttons are the size of the other
  dialogs' (the share picker's and the app shortcuts panel's): one size for every dialog.
- **Fixed:** Maximizing or snapping a window across two monitors of different sizes could send it
  to the first monitor when its middle fell beside the shorter one; it now goes to the monitor it
  is mostly on.
- **Fixed:** A window title cut short ("…") on a narrow window stayed cut once the window was
  widened or maximized, until the title changed.
- **Fixed:** The capture prompt's question was cut short ("…") for programs with names as long as
  `wf-recorder`. It now reads "Allow wf-recorder to capture your screen?" (the line below still
  says it can see any window), and the prompt is a little wider, so it fits whole for names as
  long as `gpu-screen-recorder`.
- **Fixed:** Editing `[colors]` while the share picker, the color picker, the apps' shortcuts panel or
  the region guide was open left their text in the old colors until they were opened again.
- **Fixed:** When another program already owned `org.freedesktop.ScreenSaver`, Pila left its
  interface registered on its own D-Bus connection anyway; it now removes it.
- **Changed:** A new default wallpaper: the plain `background` with a small pile of three bars in
  the bottom-right corner (halfway to `hover`, `hover`, and the `snap-preview` accent on top), in
  place of the outlined window stack. Still drawn from `[colors]`.

## 0.9.8 (2026-11-26)

- **New:** Mod+Shift+E asks before quitting, in a dialog (Enter, Mod+Shift+E again or Quit to
  quit; Esc or Cancel to stay): the new `confirm-quit` action, now its default. `quit` still
  quits at once, for `pila msg action quit` and logout menus; a config that binds
  `"Mod+Shift+E" = "quit"` keeps doing so.
- **New:** Direct screen capture permissions (`[permissions] enforce = true`, off by default):
  programs capturing through wlr-screencopy or ext-image-copy-capture (grim, wf-recorder) are
  allowed or denied by `[[permission]]` rules on their executable path, or asked with a prompt
  (Allow, Deny, Remember); `pila msg capture-permissions` lists what's allowed and
  `pila msg forget-capture-permission <id>` (or `all`) revokes it. Portal sharing is unchanged.
- **New:** Apps that ask over D-Bus to keep the screen on (`org.freedesktop.ScreenSaver`'s
  `Inhibit`, as Firefox, Chromium, mpv and call apps do, and Flatpaks through the portal) now
  keep swayidle from locking the session and turning the monitors off, as apps using the Wayland
  idle-inhibit protocol already did. Pila's log names each app that asks.
- **Changed:** Pila's panels no longer show key hints (the welcome screen, the quit dialog, the
  capture prompt, the share picker, the app shortcuts panel, the shortcuts cheatsheet, the color
  picker and region banners), for a cleaner look. The keys work as before and are in the
  manual. The cheatsheet is shorter, without its footer.
- **Fixed:** A program could answer Pila's screen share picker or global shortcuts panel by
  typing on a virtual keyboard (as `wtype` does), sharing the screen with itself or confirming
  its own shortcuts. Keys from virtual keyboards now answer none of Pila's prompts; the
  keyboard and the pointer still do.
- **Fixed:** `pila msg forget-screen-share` answered "forgot 0" before Pila had read the
  remembered shares (it does once it serves the portal), and those read after were back; it now
  fails, saying why.
- **Fixed:** A remembered screen share forgotten right after one was remembered (or app
  shortcuts changed twice in quick succession) could be back after a restart: two saves of the
  same file could finish in the wrong order, or write the same temporary file at once. Saves
  are now written in the order they were made.
- **Fixed:** With `hot-corner-delay` set, the overview could open after the pointer had left the
  corner by a jump rather than a move (`focus-output`, or the corner's monitor unplugged).
- **Fixed:** The hot corner, a swipe or `pila msg` could open the overview behind the screen
  share picker or the global shortcuts panel.
- **Fixed:** A monitor asked to power on while Pila's session was on another VT, and then off
  again (swayidle's timers keep running there), still came on when the session came back.
- **Fixed:** A monitor whose DisplayPort link went bad and that couldn't be turned back on stayed
  off for good; it now comes back with input or `power-on-monitors`, and power clients are told
  it's off.
- **Fixed:** A monitor set up again on a CRTC with another gamma size lost its night light
  (wlsunset) until the client set its ramps again on its own; the client is now told at once,
  so it does.
- **Fixed:** wf-recorder could quit while recording (with an mp4 file) when another capture
  started, such as a portal share's picker or grim: a second frame copied in the same refresh
  was presented at the same time as the first, which the recorder refused. Captures now get one
  frame per refresh of their output.
- **Fixed:** A window restored from full screen or maximized next to another monitor could leave
  a strip of its shadow on that monitor until something else redrew it (since 0.9.7): stretched
  with the window, the shadow reached further than the monitors asked to redraw.

## 0.9.7 (2026-11-24)

- **New:** The overview's hot corner can be any corner of the screen (`[overview]
  hot-corner-position`), and made harder to hit by accident: `hot-corner-pressure` asks for the
  pointer to be pushed that many logical pixels past the corner (with a mouse or touchpad), and
  `hot-corner-delay` for it to stay there that many milliseconds. By default it's still the
  top-left corner, at once.
- **Fixed:** A monitor whose DisplayPort link went bad (after a resume, or through a dock) while
  Pila's session was on another VT could be left off; it's now set up again when the session is
  back. And when the link could no longer carry the monitor's mode, the monitor stayed off: it
  now comes on with a lighter mode, and windows and panels follow it.
- **Fixed:** A monitor powered on (`pila msg action power-on-monitors`, `wlopm`) while Pila's
  session was on another VT stayed off; it now comes on when the session is back.
- **Smithay updated** (10 commits to `54cf9c10`, fork rebased; 0.9.6's pin preserved as tag
  `rev-afdce320`): drops the fork's zero-width text-cursor patch (now upstream) and stops
  re-broadcasting unchanged selections.
- **Fixed:** A newly allocated 1440p shm buffer (such as `swaybg`'s first wallpaper on a 2560×1440
  monitor) could still spike to ~15 ms on AMD APUs (`radeonsi`) with 128 KiB strips and a cold
  upload path. Pila now warms up `glTexSubImage2D` at renderer startup and uploads shm buffers in
  64 KiB strips, keeping 1440p uploads under 8 ms (and logs any upload over 8 ms as `slow upload`).
- **Improved:** Lowered CPU overhead in the event loop (`after_frames`), per-frame scene building,
  and multi-monitor redraws: housekeeping skips animation stepping, slide-pool trimming, capture
  timing, and hidden-surface scans when nothing needs them; focus changes, animations, fill states,
  and unknown-role surface commits only redraw affected outputs; `Pila::scene` caches decomposed
  blur regions per surface and skips full hit-testing when the pointer isn't over any title bar.

## 0.9.6 (2026-11-24)

- **New:** Screen sharing can be remembered for an app. When an app asks the portal to remember its
  share (Kooha and OBS do), the share picker shows a **Remember** toggle (`R`, or a click; off by
  default). With it on, the app gets the same monitor, region or window again next time without the
  picker; if that isn't there, the picker asks again. `pila msg screen-shares` lists what's
  remembered and `pila msg forget-screen-share <id>` (or `all`) revokes it. Grants meant to last
  are kept in `~/.local/state/pila/screen-shares.json`.
- **Fixed:** Recording a shared region in full-screen mode failed in Kooha (a missing GStreamer
  plugin, it said) when the region had an odd width or height in pixels, which video encoders such
  as x264 refuse. A region is now shared with even sides: one rounded down loses its last column
  or row, and the guide shows the size shared.
- **Fixed:** A share picker, color picker or app shortcuts panel opened in Pila's first moment
  (an app started with the session asking right away) could stay without its text until it was
  opened again, and the shortcuts cheatsheet could take the keys without showing. They now get
  their text as soon as the fonts have loaded.
- **Fixed:** A config with a NUL character (`"\u0000"`) in a key of a binding or in `[keyboard]`
  crashed Pila at startup, on a reload, and `pila --check-config`; such a file is now refused
  with a message. So is a NUL in a key remembered in `app-shortcuts.json`, which crashed Pila
  when the app asked for its shortcuts.
- **Fixed:** A key written by its code (`Ctrl+0x41`) in a binding, or in an app's global shortcut,
  never fired when it named a capital letter. It now stands for the key, as its name does.

## 0.9.5 (2026-11-23)

- **Security (lock screen and screen sharing):** If two apps asked to share the screen at once,
  locking the session showed the second request behind the lock screen, where Enter or a click
  shared the screen (or took a screenshot) without the user seeing what was asked. Locking now
  cancels every pending request, Pila's panels take no input while locked, and nothing is shared
  while locked.
- **Security (screen sharing):** An app receiving a screen share could crash Pila, and with it the
  session, by shrinking a shared-memory buffer (`SIGBUS`). The buffers are now sealed at their size.
- **Fixed:** Locking while `Mod+Tab` was held left the switcher open underneath, and a click on the
  lock screen could activate a window. Locking now ends the switch.
- **Fixed:** Every screenshot (`grim`, the portal's) and the first frame of every screen share
  into shared memory held Pila up to 9 ms at 1920x1080, which could make a monitor miss a
  refresh. The pixels are now written into the app's buffer on a thread of their own.
- **Fixed:** A blur region made of very many rectangles cost several milliseconds of CPU on every
  frame; past 64 rectangles it's now blurred as its bounding box.
- **Fixed:** Nested subsurfaces could be placed past Pila's 32768 px limit by moving a parent back
  without applying it.
- **Fixed:** With `gaps`, the shadow of a maximized window that reached the next monitor wasn't
  redrawn there, appearing late and staying behind after the window closed.
- **Fixed:** Taskbars using `ext-foreign-toplevel-list` didn't get title or app id changes from
  minimized windows or windows on other workspaces.
- **Fixed:** Windows brought onto the current workspace by lowering `workspaces` were stacked above
  always-on-top windows.
- **Fixed:** Games running through Xwayland that lock the pointer could make it jump when
  unlocking (Pila ignored their pointer position hint).
- **Fixed:** The share picker read desktop entries outside the data directories for app ids
  containing a path, and could block on a FIFO.
- **Fixed:** `pila-session` didn't remove `XAUTHORITY` from the systemd user environment when the
  session ended.
- **Fixed:** Two screenshots saved at the same moment (two apps asking) could write over each
  other, so an app got another's picture. Each is now its own file, readable by the user alone.
- **Fixed:** A remembered global shortcut read back from `app-shortcuts.json` was bound even if no
  app could be given that key (a plain letter); such an entry is now dropped and the app is asked
  again.
- **Fixed:** A share or screenshot asked while the color picker or an app's shortcuts panel was
  open showed its panel over it; it now waits for the other to close. The color picker no longer
  opens over the shortcuts panel.
- **Fixed:** An X11 menu or tooltip could end up below a window opened or raised while it was
  shown.
- **Fixed:** Maximizing on an output too short for `gaps` and a title bar asked the window for a
  negative height.
- **Fixed:** A window held (pressed or dragged) in the overview that closed meanwhile stayed held.
- **Fixed:** Session restore accepted a toplevel renamed to nothing, created sessions past its cap
  when all were in use by several apps, and restored sizes from `sessions.json` that no window
  could have (a negative one crashed debug builds).
- **Fixed:** The bell's flash redrew every monitor instead of the ringing window's.
- **Fixed:** `pila msg --json` passed DEL and C1 control characters from window titles to the
  terminal as they were; they're now escaped.

## 0.9.4 (2026-11-23)

- **Share a screen region:** The share picker now includes **Region** after the monitor list for
  both screen sharing and app screenshots. Picking it hides the panel and turns the pointer into a
  crosshair: drag a rectangle on a monitor (with a top banner showing its size) and release to
  share it, or press Escape to return to the picker. The app sees it as a fixed monitor of that
  size showing whatever appears inside the area. While active, a thin accent-colored outline marks
  the shared region on screen without appearing in the stream.
- **Tabbed share picker:** When an app supports window sharing (and for all screenshots), the
  picker splits into two tabs, **Screens** (monitors and region) and **Windows**, instead of showing
  everything at once. Switch tabs with `Tab`, `Shift+Tab`, or a click; each tab remembers its
  selection and the panel stays centered without jumping.
- **Fixed:** GStreamer-based recorders (such as Kooha) recorded 0-second clips during screen
  sharing; Pila's PipeWire frames now include presentation timestamps (`SPA_META_Header`).
- **Fixed:** The AUR `pila-git` package failed to link under `makepkg`'s default LTO flag
  (undefined `spa_*` symbols); LTO is now disabled in its `PKGBUILD`.

## 0.9.3 (2026-11-22)

- **Fixed:** Launching Pila without `XDG_RUNTIME_DIR` (e.g., via `sudo` or outside a login
  session) crashed with a backtrace; it now prints a clear error explaining what is missing and
  how to launch it.
- **Fixed:** `scripts/tty` failed with `"Permission denied"` writing `pila.portal` when Pila was
  also installed system-wide.
- **X11 app names in the share picker:** The share picker now identifies X11 apps (such as Chrome,
  Discord, or OBS running under Xwayland) by their requesting window, matching Wayland apps.

## 0.9.2 (2026-11-22)

- **Restored `.deb` and `.rpm` packages:** 0.9.1's packages failed to build due to a packaging
  error left over from dropping `xdg-desktop-portal-wlr`. 0.9.2 is identical to 0.9.1 with working
  packages (see 0.9.1 below for changes). CI now builds distro packages on every change to catch
  this before releases.

## 0.9.1 (2026-11-22)

- **Single-window screen sharing:** Apps that support window sharing (browsers, video calls, OBS)
  now see all windows in Pila's share picker below the monitors (ordered by recent use, including
  minimized and other-workspace windows) as live previews. Shared windows stream at their own size,
  keep updating while covered, minimized, or on another workspace, pause updates while locked, and
  end cleanly when closed. `xdg-desktop-portal-wlr` is no longer needed to share individual
  windows.
- **Built-in app screenshots:** Portal screenshot requests (browsers, Flatpaks) are now handled by
  Pila. Apps permitted to capture without prompting receive all monitors laid out together;
  otherwise, the picker lets you choose a monitor or a single window (even covered or minimized)
  and click **Take**. Screenshots omit the pointer and are stored in
  `$XDG_RUNTIME_DIR/pila-screenshots` (last 10 kept). If you maintain your own `pila-portals.conf`,
  set `org.freedesktop.impl.portal.Screenshot=pila`.
- **Unsandboxed app names in the share picker:** Apps outside a sandbox that provide a parent
  window handle now show their actual app name instead of `"An app asks to share a screen"`.
- **Dropped `xdg-desktop-portal-wlr`:** With screen sharing, window sharing, and app screenshots
  built into Pila, packages no longer recommend `xdg-desktop-portal-wlr` and Pila no longer
  installs `/etc/xdg/xdg-desktop-portal-wlr/pila` (`scripts/tty` stops writing
  `~/.config/xdg-desktop-portal-wlr/pila`, leaving any existing file alone). You can still opt back
  in via your own `pila-portals.conf`.
- **App global shortcuts:** Apps requesting global shortcuts via the desktop portal (push-to-talk,
  recorder hotkeys) now work across all windows. On first request, a panel lets you confirm or
  rebind each shortcut (Backspace unbinds; keys used by Pila or another app are rejected) and
  persists your choices per app. Bound keys go only to that app and are disabled while locked. If
  you maintain your own `pila-portals.conf`, add
  `org.freedesktop.impl.portal.GlobalShortcuts=pila`.
- **Screen color picker:** Portal color picker requests (GTK color chooser, GIMP, Inkscape) switch
  the pointer to a crosshair with a top banner; click any pixel to sample or press Escape to
  cancel without leaking clicks to apps.
- **Upfront build-dependency checks in `scripts/install`:** Checks for `pkg-config`, required
  `-dev`/`-devel` libraries, and `libclang` before invoking Cargo, listing missing dependencies
  clearly instead of failing deep inside `bindgen`.

## 0.9.0 (2026-11-22)

- **Built-in screen sharing:** Browsers, OBS, and video call apps now share screens directly
  through Pila's own `xdg-desktop-portal` backend (`pila.portal`) over PipeWire, replacing
  `xdg-desktop-portal-wlr` for screen casts. When requested, a modal picker names the app and
  displays live monitor previews (click or arrows, Share/Enter, Cancel/Escape; locking cancels the
  prompt). Frames render directly into client dmabufs when supported (~0.2 ms CPU/frame at 1080p),
  falling back to shm read-back, and adapt to monitor rotation or scaling. Pila now depends on
  `libpipewire` (and `libclang` at build time).
- **Unified panel styling:** The window menu now matches the window switcher, shortcuts
  cheatsheet, and share picker (shared background, border, rounded corners, accent hover highlight,
  consistent title layout, and footer key hints). The cheatsheet and welcome screen dim all
  monitors while visible, matching the share picker.
- **Fixed:** The window menu now renders above the shortcuts cheatsheet if both are open (e.g., via
  `pila msg action show-shortcuts`).
- **Capture copy budgets in `pila-check`:** `pila-check` now checks shm capture read-backs
  (`copies` in frame stats, ≤ 8 ms each), since they run on the event loop like shm uploads.

## 0.8.3 (2026-11-22)

- **Gaps:** Added `gaps` (logical pixels, default `0`) to leave space around maximized and snapped
  windows and between snapped halves/quarters. When non-zero, maximized windows keep their shadow
  and rounded corners; fullscreen windows still cover the full monitor.
- **Snapped windows respect new bars:** Snapped windows now reposition below newly opened or
  resized bars (such as `waybar` starting after an app opens at login), matching maximized windows.
  Refitting a maximized window to a bar no longer raises it above other windows.
- **Striped shm uploads:** CPU-drawn client buffers (`foot`, `swaybg`, software Xwayland apps) now
  upload to the GPU in strips instead of all at once. On AMD, full-frame 1080p uploads previously
  spiked to 10–22 ms about a third of the time, dropping a frame when opening fullscreen `foot`;
  striped uploads stay under 5 ms (`pila-check` now enforces an 8 ms cap).
- **Direct-scanout diagnostics:** `pila::frames` now logs why a fullscreen buffer was rejected for
  direct scanout (format, modifier, and whether scanout dmabuf feedback was sent). On AMD with
  Mesa 26.3, OpenGL apps keep a compressed layout the display engine rejects (tracked in
  `docs/ROADMAP.md`).

## 0.8.2 (2026-11-21)

- **Security (no abstract X11 sockets):** Xwayland previously listened on an abstract Unix socket
  with no filesystem permissions, allowing sandboxed Flatpak apps (with network access but no X11
  permission) or other local users to access X11 clipboards and inject keystrokes. Pila now binds
  only the filesystem socket in `/tmp/.X11-unix`.
- **Security (Xwayland cookie auth):** Xwayland now runs with `-auth`. Pila generates a
  per-session user-only `XAUTHORITY` cookie in `$XDG_RUNTIME_DIR` and passes it to child processes
  and the TTY D-Bus session bus, rejecting unauthorized X11 connections.
- **Security (no focus stealing on hover):** Apps could previously mint an `xdg-activation` token
  from a mere pointer-enter serial and steal keyboard focus while hovering. Only a click qualifies
  now, and only until keyboard focus moves elsewhere.
- **Blur behind app menus:** Added window-rule options `popup-blur` (`true`/`false`) and
  `popup-corner-radius` to blur behind menus and popups that don't request blur themselves (such as
  GTK menus), following their rounded corners and popover shapes while skipping client-side
  shadows. Works alongside `opacity`.
- **Faster shm uploads:** CPU-rendered buffers (`swaybg`, `foot`, software Xwayland apps) now
  upload to the GPU on commit instead of mid-frame, dropping the initial wallpaper frame from up to
  19 ms to under 4 ms and cutting fullscreen `foot` scrolling overhead roughly in half at 1440p.
  Tracked under `uploads` in `pila::frames` and verified by `pila-check`.
- **For packagers (`zbus` 6):** Upgraded `zbus`, dropping unmaintained `zvariant`/`zbus_names`
  versions flagged by `cargo audit` (`RUSTSEC-2026-0370`–`0372`) and removing 14 build crates. No
  new system libraries.

## 0.8.1 (2026-11-20)

- **Overview:** Added the `overview` action (`Mod+W`, top-left hot corner via
  `[overview] hot-corner`, or four-finger swipe up; `overview open` / `overview close` for explicit
  control). Spreads workspace windows (including minimized ones) live across their monitors without
  overlap over a dimmed wallpaper while keeping bars visible, with a live top workspace strip for
  switching workspaces or dragging windows between them. Animated via `[animations] overview`
  (250 ms).
- **Fixed:** Exiting a fullscreen tearing game on bare metal caused frame stats (`scripts/check`
  and `pila::frames`) to falsely count the torn vblanks as missed refreshes.

## 0.8.0 (2026-11-20)

- **GTK-style rounded top corners by default:** Server-side decorated windows now default to 8 px
  rounded top corners and square bottom corners (`corner-radius = 8`, `rounded-corners = "top"`).
  Use `rounded-corners = "all"` for all four corners or `corner-radius = 0` for square windows
  (existing configs generated by earlier versions explicitly set `corner-radius = 0`; delete that
  line to adopt the new default). Blur behind all-rounded apps (like libadwaita) rounds all four
  corners automatically.
- **Built-in wallpaper for `swaybg`:** The default `autostart` now runs `swaybg` with a minimalist
  wallpaper generated from `[colors]` (regenerated on color changes, scaled with `-m fit`). Replace
  with `swaybg -i FILE -m fill` for your own image, or add `"swaybg"` to existing custom
  `autostart` lists. Included in `pila-desktop`.
- **Welcome overlay and shortcuts cheatsheet:** First-run setups now show a brief welcome overlay
  listing essential shortcuts as keycaps reflecting your active bindings (dismissed by any key or
  click, while still executing pressed shortcuts). Press `Mod+?` (`Mod+Shift+slash`, action
  `show-shortcuts`) anytime for the full categorized cheatsheet.
- **`pila-desktop` metapackage:** Added `pila-desktop` (APT, DNF, AUR, Nix) bundling Pila with all
  default desktop tools, while `pila` itself now recommends only core session packages (Xwayland,
  portals, `xdg-utils`). On Debian/Fedora, install `pila-desktop` to keep those tools on upgrade.
  Missing programs from the default `autostart` are now skipped quietly instead of notifying.
- **Default idle locking and monitor power-off:** Default `autostart` now runs `swayidle` to lock
  with `swaylock` after 5 minutes idle and before sleep, and power off monitors after 10 minutes
  (waking on any input; inhibited by visible video playback). Included in `pila-desktop`.
- **Replaced bundled tools with standard Wayland tools:** Removed `pila-launcher` and `pila-bar`;
  defaults are now `fuzzel` and `waybar`. `scripts/install` and `scripts/uninstall` clean up old
  binaries, and configs still referencing them receive a missing-program notification.
- **Matching default tool themes (`tool-configs`):** Unmodified default tools (`waybar`, `mako`,
  `fuzzel`, `swaylock`, `swaybg`) automatically load Pila-generated configs from
  `$XDG_RUNTIME_DIR/pila-tools` matching `[colors]`, the title bar font, and 8 px popup corners
  whenever you have no config of your own, leaving `~/.config` untouched. `waybar` displays app
  icons and Font Awesome symbols when available (`pila-desktop` includes it), and `fuzzel` scales
  consistently across mixed-DPI monitors. Disable with `tool-configs = false`. `mako` joins the
  default `autostart`.
- **System sans-serif font:** Title bars and the window switcher now use fontconfig's `sans-serif`
  (`fc-match sans-serif`) instead of hardcoding Open Sans.
- **Fewer external dependencies:** Pila now sends notifications directly over D-Bus (dropping
  `notify-send`), `pila-report` identifies GPUs from `/sys` and `pci.ids` (dropping `lspci`), and
  TTY sessions export environment variables directly over D-Bus (dropping
  `dbus-update-activation-environment`).
- **Missing-program notifications:** When a bound key, terminal, launcher, or custom `autostart`
  command fails because a binary isn't installed, Pila now shows a one-time notification naming
  the missing program.
- **Default volume, brightness, and media keys:** Bound by default to `wpctl`, `brightnessctl`, and
  `playerctl` (with key repeat enabled for volume and brightness), included in `pila-desktop`.
- **Visual window switcher:** Holding `Mod+Tab` or `Alt+Tab` briefly now opens a centered window
  switcher with live previews (including videos, minimized windows, and other-monitor windows),
  titles, and click-to-select, while quick taps still switch immediately without flashing the UI.
  Rendered directly by Pila with matching rounded corners and no external tools or screen capture.
- **Window switcher over IPC:** Added `pila msg switch` and the `switch` subscription event
  (`pila msg subscribe switch`), exposing the active switcher window list (most recently used
  first), current selection, and `null` when idle for external switchers.
- **`pila msg colors`:** Lists active `[colors]` values, and `pila msg subscribe colors` streams
  changes on config reload so external bars and launchers can match Pila's theme.
- **Fixed:** Nested Pila previously stayed alive after its host compositor exited or crashed,
  printing `Io error: Broken pipe` in a loop. It now exits cleanly with `"lost the host compositor"`.
- **Signed APT and DNF repositories:** `.deb` and `.rpm` packages are now published in signed
  repositories on GitHub Pages for automatic upgrades on Debian, Ubuntu, and Fedora. `.rpm` builds
  now target the oldest supported Fedora release (44, glibc 2.43) so a single package covers all
  supported Fedora versions.

## 0.7.1 (2026-11-18)

- **Step-by-step half and quarter snapping:** `Mod` + arrow keys now use the new
  `snap <left|right|up|down>` action: `Mod+Left` snaps to the left half, `Mod+Up`/`Mod+Down` narrow
  a half into its top or bottom quarter, `Mod+Right` shifts a quarter across, opposite vertical
  arrows expand back to a half, pressing toward an already-snapped edge restores, and `Mod+Up` from
  a top quarter maximizes. On floating windows, arrows keep their previous half/maximize/restore
  behavior. Existing generated configs keep their old bindings (see the manual to restore the old
  mapping).
- **Systemd user session integration:** On systemd systems, `pila-session` now runs Pila as
  `pila.service` bound to `graphical-session.target`, starting enabled graphical user services and
  XDG autostart entries (`~/.config/autostart`) once Pila is ready, stopping them on exit, and
  cleaning up exported env vars afterward. Second concurrent sessions are refused. Set
  `PILA_NO_SYSTEMD=1` to bypass systemd; non-systemd systems are unchanged. Units install to
  `/usr/lib/systemd/user` (`SYSTEMDUSERUNITDIR` in `scripts/install`).
- **`[environment]` section:** Configure environment variables for everything Pila launches
  (`NAME = "value"` to set, `NAME = false` to unset; applied on save to newly launched apps and
  exported to D-Bus and `systemd --user` on TTYs). Pila-managed variables (`WAYLAND_DISPLAY`,
  `DISPLAY`, etc.) cannot be overridden and trigger a warning. Ships Wayland-first defaults for Qt
  (`QT_QPA_PLATFORM = "wayland;xcb"`), Electron (`ELECTRON_OZONE_PLATFORM_HINT = "auto"`), older
  Firefox (`MOZ_ENABLE_WAYLAND = "1"`), and Java X11 (`_JAVA_AWT_WM_NONREPARENTING = "1"`).

## 0.7.0 (2026-11-18)

- **`pila-launcher`:** Added a built-in app launcher (written in `iced`, software-rendered) listing
  installed `.desktop` apps with icons and live search, staying warm in the background after first
  launch, supporting `fcitx5`, and opening terminal apps in `kitty` (or `--terminal`). Set as the
  default `launcher` (`Mod+D` and tapping `Mod`).
- **`pila-bar`:** Added a built-in top/bottom bar (`iced`, layer-shell + `ext-workspace`) showing
  workspaces (click/scroll to switch) and a localized clock (`--bottom`, `--clock FORMAT`). Set as
  the default `autostart`.
- **Larger title bar text:** Increased title bar font size from 13 to 14 logical pixels to match
  `pila-launcher` and `pila-bar`.
- **No auto-opened terminal at login:** Default startup now launches `pila-bar` instead of a
  terminal (use `Mod+Return` or add a terminal to `autostart`). Set `launcher = ""` and
  `autostart = []` to disable both.
- **Defaults moved to `config.example.toml`:** All defaults now live as active lines in
  `config.example.toml` (embedded in Pila) rather than hardcoded in Rust. Omitted keys fall back to
  the embedded example, while first-run configs copy the example so existing choices stay stable
  across upgrades.
- **`default-bindings = false`:** Disables all built-in bindings and gestures so only config-defined
  ones apply (for custom setups or `greetd` greeter sessions; VT switching remains active).
  Requested in discussion #6.
- **Cursor theme and size in config:** Added `theme` and `size` under `[cursor]` (taking precedence
  over `XCURSOR_THEME`/`XCURSOR_SIZE`), live-reloaded and exported to launched apps. Requested in
  discussion #7.
- **X11 launchers (`dmenu`) receive keyboard focus:** Non-menu override-redirect X11 windows that
  take input now receive keyboard focus even over Wayland windows and restore focus on close, while
  X11 menus and tooltips keep their non-focusing behavior. Reported in issue #5.
- **Removed shell-specific guides and helpers:** Removed `scripts/shells`, shell-specific manual
  guides, and the old screenshot/demo scripts in favor of a generic full-desktop-shell guide.
  `scripts/check` now uses `waybar` for its layer-rule check.

## 0.6.1 (2026-11-17)

- **Lower display latency on TTYs (`render-late`):** In steady scenes, Pila now delays drawing each
  frame until just before the upcoming vblank (based on recent CPU/GPU frame times, floored at half
  a refresh period), letting late client commits and pointer motion hit the next refresh. Pauses
  during animations, brief post-open/resize settling, tearing, and VRR, and backs off for 2 s if a
  frame misses. Cut redraw-to-vblank latency on the AMD test laptop from 25.8 to 21.3 ms (internal)
  and 28.5 to 21.7 ms (external) with zero dropped refreshes. Disable with `render-late = false`.
- **GPU frame timing and latency stats:** `RUST_LOG=pila::frames=debug` now measures per-frame GPU
  time via native fences without blocking (`gpu_avg_ms`, `gpu_max_ms`, trace-level per-frame logs),
  plus TTY display latency and late-render margins (`pila-check` reports them; pass
  `PILA_CHECK_RENDER_LATE=false` to compare). Also fixed double-counting frames in `pila-check` when
  GPU lines are present and false missed-refresh counts under late-rendering nested hosts.
- **Smithay updated** (1 commit to `33323a9`, fork rebased; 0.6.0's pin preserved as tag
  `rev-12a26c10`): NUL-terminates keymaps for pre-v7 `wl_keyboard` clients such as Xwayland.
- **Window coordinates in IPC events:** `pila msg subscribe` `windows` events now include `x`, `y`,
  `width`, and `height` matching `pila msg windows`. Dragged/resized windows emit a single update on
  release rather than per frame.
- **`pila msg minimize <id>`:** Minimizes a window by ID (`pila msg focus <id>` restores it),
  refused while locked.
- **Per-output and unfocused window rules:** Added `open-on-output` (matching connector or
  `"Make Model"`, falling back to the pointer's monitor) and `open-focused = false`. Nested Pila now
  sets app ID `pila` so rules can route dev instances out of the way.
- **Cached blur backdrops for hidden windows:** Minimized and other-workspace blurred windows now
  keep their backdrop textures until closed instead of reallocating on return (fixing a dropped
  frame on battery).
- **Reduced blur texture churn during resizes:** Blur backdrops now allocate with growth headroom
  and shrink only below half capacity, cutting allocations during resizing animations (e.g., 6 down
  to 3 when opening a shell control center).

## 0.6.0 (2026-11-16)

- **Screen tearing for games (`wp-tearing-control`):** Fullscreen games with vsync off (Vulkan
  immediate present mode) can now present via async page flips on TTYs when supported by the driver.
  Disable with `tearing = false`. Counted separately as `torn` in frame stats and exercised with
  `vkcube` in `pila-check`.
- **On-demand Xwayland with optional idle shutdown:** Pila reserves the X display at startup (so
  `DISPLAY` is set immediately without blocking startup apps) and launches Xwayland only when the
  first X11 client connects. Optional `[xwayland] idle-timeout = <seconds>` shuts Xwayland down when
  no X11 clients remain. Xwayland also now inherits Pila's full environment (such as `MESA_*` and
  `__GLX_VENDOR_LIBRARY_NAME`).
- **Disable X11 completely:** `[xwayland] enabled = false` skips reserving an X display and leaves
  `DISPLAY` unset (live-reloaded; active X11 apps keep running until closed).
- **X11 keyboard grabs (`xwayland-keyboard-grab`):** X11 VMs and remote desktops that grab the
  keyboard now receive Pila's shortcuts while focused (matching Wayland's
  `keyboard-shortcuts-inhibit`; `quit` and clicking another window still escape).
- **Per-surface opacity (`wp-alpha-modifier`):** Supports compositor-applied surface alpha for video
  players and overlays across rendering, capture, and blur.
- **Pointer warping (`wp-pointer-warp`):** Allows the focused client under the pointer (such as SDL
  3 games) to warp the pointer within its own surface (blocked while locked).
- **Popup blur:** Menus and input-method candidate windows requesting `ext-background-effect` now
  receive blur over both windows and panels (`blur = false` rules still block them).
- **Top/bottom half snapping:** Added `snap-top` and `snap-bottom` actions (unbound by default;
  repeating the same side restores).
- **Desktop tool compatibility guide:** Added tested, partially supported, and unsupported bar/shell
  details plus a `waybar` config to the manual (*Building your desktop*).
- **Paced pointer resizing:** Resize drags now keep at most one unacknowledged configure in flight
  per window so slower apps (browsers, Electron) receive the latest size without building a backlog.
- **Fixed:** Destroying an `xdg-decoration` object now permanently keeps client-side decorations
  even if the window is unmapped and remapped later.
- **Fixed (IME):** Candidate popups opened from a GTK popover text field can now be clicked without
  dismissing the popover (regression since 0.5.8).
- **For packagers:** Xwayland is now an optional dependency (`optdepends` in `pila-git`, Recommends
  in `.deb`/`.rpm`, `withXwayland ? true` in Nix). Pila's Smithay fork (`github.com/igalmarino/smithay`,
  branch `pila`, based on 2026-11-14 `main`) now includes X display reservation (PR #2269) and async
  page flips (PR #1948); earlier pins remain tagged (`rev-b8f524b3`, `rev-64a43022`).

## 0.5.9 (2026-11-15)

- **Fixed:** X11 windows setting `_NET_WM_WINDOW_OPACITY` (such as terminals or `picom-trans`) no
  longer lose transparency when `corner-radius` is enabled (regression since 0.2.0).
- **Modal X11 dialogs:** `_NET_WM_STATE_MODAL` dialogs now redirect focus away from their parent
  window, matching Wayland's `xdg-dialog`.
- **Fixed:** Automatic monitor placement next to a fractionally scaled display no longer overlapped
  by 1 logical pixel (e.g., 1280 px at 1.5x spans 854 px, not 853).
- **Fixed:** The headless backend now chooses the preferred output mode (matching bare metal)
  instead of always picking the highest refresh rate.

## 0.5.8 (2026-11-15)

- **Input methods (IME):** Added `input-method-v2` and `text-input-v3` support for `fcitx5` and
  other CJK input methods across GTK, Qt 6.7+, `foot`, `kitty`, Chromium/Electron (with
  `--enable-wayland-ime`), and launchers. Candidate popups position below/above the text cursor,
  Pila's keybindings take priority, menus work while IME holds the keyboard, and input methods are
  disabled on the lock screen. Requested in discussion #2.
- **For packagers:** Switched temporarily to Pila's Smithay fork (`github.com/igalmarino/smithay`,
  branch `pila`) carrying a fix so zero-width GTK text-cursor rectangles aren't dropped.

## 0.5.7 (2026-11-14)

- **Fixed (GTK 4):** Opening a dropdown or menu a second time no longer froze GTK 4 apps. GTK
  destroys a hidden popover's popup and reuses the same surface for the next one, which Pila had
  misread as an unmap commit and never configured, leaving a stuck grab. Reported in HandBrake and
  a text editor (regression since 0.4.2).
- **Repeatable keybindings:** Bindings written in table form with `repeat = true` (e.g.,
  `"XF86AudioRaiseVolume" = { action = "spawn wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+", repeat = true }`)
  now repeat while held using `[keyboard]`'s `repeat-delay` and `repeat-rate` (for keys only, not
  wheel notches or modifier taps). Plain string bindings still trigger once per press.
- **Startup Num Lock:** Added `numlock = true` under `[keyboard]`, applied at startup and restored
  after reloading a new keymap.
- **Mouse-wheel binding cooldown:** Added `[input] wheel-binding-cooldown` (default 150 ms; `0`
  disables) so a fast wheel flick switches one workspace instead of several, dropping intermediate
  notches rather than forwarding them to apps.
- **Duplicate `[[output]]` warning:** Defining two `[[output]]` blocks for the same monitor (where
  only the first applied) now prints a warning in `pila --check-config` and notifies on startup and
  reload.

## 0.5.6 (2026-11-14)

- **Fixed double rounded corners on GTK apps:** Global `corner-radius` now skips apps that draw
  their own client-side shadows and corners (per-app window rules still override this). Reported by
  a tester.
- **Fixed stuck keyboard focus after panel menus:** Closing a dock or panel menu by clicking empty
  desktop now ends its keyboard grab and returns focus to a window instead of leaving focus on a
  non-keyboard panel. Caught by the new input-invariant checks.
- **Fixed quick unmaximize-to-fullscreen restore size:** Going fullscreen before an app committed
  its unmaximized size now remembers the requested unmaximized size rather than the old maximized
  one.
- **Fixed snapped window drag offset:** Dragging a snapped window out of its half now stays
  properly aligned under the pointer (including GTK client-side title bar drags; regression since
  0.5.4).
- **Fixed no-op taskbar state requests:** Requesting unmaximize or exit-fullscreen via
  `wlr-foreign-toplevel` on a window not in that state no longer moves it to the default two-thirds
  centered rectangle.

## 0.5.5 (2026-11-14)

- **Fixed GTK 4 title bar drag jumps:** GTK 4 sends `unset_fullscreen` when starting a title bar
  drag even on non-fullscreen windows; no-op fill-state requests now leave window geometry
  unchanged (still sending a configure per protocol).
- **Auto-generated starter config:** On first launch without `~/.config/pila/config.toml` (or
  `$XDG_CONFIG_HOME/pila/config.toml`, unless `--config` is passed), Pila writes a starter config
  from `config.example.toml` without overwriting existing files or symlinks.
- **Fixed lost click after dock/panel menus:** Ended finished popup grabs immediately instead of
  consuming the next input event, and restored keyboard focus to windows after non-keyboard panel
  menus close.
- **Fixed per-monitor taskbars for hidden windows:** Minimized and other-workspace windows now
  keep reporting their target monitor to taskbars instead of disappearing from per-monitor lists.

## 0.5.4 (2026-11-14)

- **Mouse-wheel keybindings:** `Mod` + mouse wheel now switches workspaces (`Mod+Shift` + wheel
  moves the focused window). Any binding can use `WheelScrollUp`, `WheelScrollDown`,
  `WheelScrollLeft`, or `WheelScrollRight` (unbind with `"none"` to pass scrolls back to apps).
- **Drag threshold before restoring filled windows:** Clicking the title bar (or `Mod` + clicking)
  on a maximized, fullscreen, or snapped window no longer immediately restores it until the pointer
  moves a few pixels.

## 0.5.3 (2026-11-13)

- **Eliminated AMD direct-scanout stalls:** Switched Pila's DRM swapchain from `ARGB8888` to opaque
  `XRGB8888` to match primary-plane client buffers, eliminating `amdgpu`'s 11 to 14 ms global plane
  reset stall when entering or leaving direct scanout on dual-monitor setups.
- **Improved `pila-check` portability:** Fixed automatic exit when Pila runs as Nix's
  `.pila-wrapped` and added a fallback when Python lacks `os.memfd_create` (both found during an
  NVIDIA GTX 1060 NixOS test run where everything else passed).

## 0.5.2 (2026-11-12)

- **Installed `pila-check`:** Distro packages, Nix, and `scripts/install` now install
  `scripts/check` as `pila-check` (helpers in `share/pila/check`, logs in `~/.local/state/pila`) so
  testers can benchmark hardware without cloning the repo.
- **Project logo and session icon:** Added the stacked-window Pila logo (`docs/logo/`) to the
  README, manual, and `hicolor` icon theme (`Icon=pila` in `pila.desktop`).
- **Reduced CPU usage during mouse motion, resizes, and focus switches:** Batched event-loop
  housekeeping per iteration (avoiding 1000 Hz mouse overhead), cached title textures across resizes
  and focus toggles, skipped redundant redraws for client-handled gestures and unchanged cursors,
  and limited blurred/animated rendering to intersecting outputs. Added `loop` stats to
  `pila::frames`.
- **Warm-up at startup:** Pre-compiled shaders and warmed up window draw paths on renderer creation
  so the first opened window matches subsequent frame times (1.4 ms vs. 2.6 ms on AMD).
  `pila::frames` now logs `others_pending` on direct-scanout transitions.
- **Clearer binding error messages:** Unknown actions in `[bindings]` and `[gestures]` now print
  the key combo as written in the config file.
- **Added `docs/HOW-PILA-IS-MADE.md`:** Documented Pila's AI-assisted workflow, decision ownership,
  verification practices, and past bugs (linked from README).
- **Added *Looks* section to the manual:** Dedicated guide for colors, corners, shadows,
  animations, blur, and opacity.

## 0.5.1 (2026-11-11)

- **Shake-to-find cursor:** Rapidly shaking the mouse or touchpad temporarily magnifies the cursor
  up to 3x using sharp larger cursor-theme images (also works on the lock screen; configurable via
  `[cursor] shake-to-find` and `shake-magnification`).
- **Pre-allocated workspace slide textures:** Pooled two output-sized textures per monitor between
  switches (when workspace animations are enabled) so the first frame of a slide never drops a
  refresh.
- **Direct scanout:** Fullscreen games and videos covering a primary-GPU monitor on a TTY now scan
  out directly without compositing (`direct-scanout = false` disables).
- **Fixed cross-monitor redraws from maximized/fullscreen windows:** Removed unused shadow margins
  from maximized and fullscreen damage rects so they stop waking adjacent monitors.
- **Experimental multi-GPU support (`other-gpus = true`):** Renders secondary-GPU outputs on the
  primary GPU and copies frames across, including GPU hotplug. Disabled by default until tested on
  real dual-GPU hardware (`pila-report` lists GPUs and secondary outputs).
- **Fixed delayed X11 window display:** Redrew immediately when Xwayland associates a `wl_surface`
  after mapping so new X11 windows don't appear blank until the next unrelated redraw.
- **Smithay updated** (10 commits to `3a08ff9`): Fixed ICCCM `UnmapNotify` withdrawal, X11
  reparenting unmap loops, and debug crashes on negative client damage sizes.

## 0.5.0 (2026-11-10)

- **Tap `Mod` alone for the launcher:** Pressing and releasing `Mod` (Super on a TTY) without any
  other key, click, scroll, or gesture opens the launcher. Disable with `"Mod" = "none"` or bind
  any standalone modifier (`"Super"`, `"Alt"`, `"Ctrl"`).
- **Default `Alt+Tab`:** Bound `Alt+Tab` and `Alt+Shift+Tab` to window switching regardless of
  `mod` (unbind with `"none"` to pass them to apps).
- **MRU fallback focus:** Closing a window, dismissing a launcher, or unlocking now focuses the
  most recently used window instead of the topmost window (preventing always-on-top windows from
  taking focus).
- **Raised open-file descriptor limit:** Pila now raises its own soft `RLIMIT_NOFILE` to the hard
  limit at startup for busy sessions while restoring the original limit for spawned child programs.
- **Fixed screencopy bounds crash:** `wlr-screencopy` regions with negative sizes or coordinates
  beyond 32,768 px now fail cleanly instead of panicking in debug builds.
- **Session lock hardening:** Enforced lock-screen isolation directly at keyboard and pointer focus
  types (catching a lingering pointer motion on lock), ignored pre-lock activation tokens while
  locked, and cancelled in-progress touchpad swipes on lock.
- **Fixed X11 popup-grab crash:** Prevented crashes when a popup grab targeted an X11 window whose
  Wayland surface wasn't associated yet.
- **IPC output power fixes:** Emitted `outputs` subscription events even when all monitors are off
  (no frames drawing), and marked powered-off monitors `[off]` in plain-text `pila msg outputs`.
- **Per-output redraws and idle throttling:** Moving the pointer or updating an app on one monitor
  no longer redraws other monitors or rebuilds their window lists, cutting multi-monitor CPU usage
  significantly. Added bare-metal idle-redraw throttling and `late` refresh tracking in
  `pila::frames`.
- **Per-scale title bar texture cache:** Mixed-DPI multi-monitor setups no longer re-rasterize
  title bars on every frame.
- **Immediate title redraw on font load:** Windows opened before fonts finished loading now show
  their titles immediately once loaded.
- **Cleaner gamma warnings:** Suppressed expected startup modeset gamma-duration warnings so
  warnings only fire for slow runtime client gamma updates.
- **Automatic crash backtraces:** Crashes now log `"pila crashed"` with a full backtrace without
  requiring `RUST_BACKTRACE` (AUR package is no longer stripped so function names appear).
- **Clearer MSRV check in AUR `PKGBUILD`:** Explicitly reports when Rust is older than 1.89.
- **Tester improvements:** `scripts/check` now enforces its own workspace count and stops cleanly
  on failed steps; `pila --version` accurately reports `-dirty`; `pila::frames` and `scripts/check`
  report `idle` redraw counts.

## 0.4.2 (2026-11-09)

- **Client geometry bounds (`MAX_EXTENT`):** Surface sizes, buffers, viewports, subsurfaces, window
  geometry, layer surfaces, and popups exceeding 32,768 px now disconnect the offending client with
  a protocol error instead of overflowing layout or rendering math.
- **Bounded monitor positions (±1,000,000):** `wlr-output-management` and `[[output]]` reject
  out-of-range monitor coordinates, and setting a head property twice now raises `already_set` per
  protocol.
- **IPC event subscriptions:** Added `pila msg subscribe` (`windows`, `workspaces`, `outputs`) so
  bars and scripts receive live updates without polling (`docs/IPC.md`).
- **Keyboard layout IPC and action:** Added `pila msg keyboard`, `pila msg subscribe keyboard`, and
  the `switch-layout <n|next|prev>` action.
- **Exported `PILA_SOCKET` to D-Bus/systemd:** Login sessions now export `PILA_SOCKET` alongside
  `WAYLAND_DISPLAY` for user-service shells.
- **Monitor power actions:** Added `power-off-monitors` (wakes on any input, consuming the waking
  key/click) and `power-on-monitors`, allowed via `pila msg` while locked, plus `power` in
  `pila msg outputs`.
- **Manual updates:** Documented shell idle/logout integration via `pila msg` and a custom Waybar
  keyboard layout module.

## 0.4.1 (2026-11-08)

- **Live hidden window shares:** Covered, minimized, and other-workspace captured windows now
  receive frame callbacks at the capture client's pace instead of throttling to 1 fps or freezing.
- **Non-blocking night light (`GAMMA_LUT`):** Gamma changes now submit as non-blocking atomic
  commits (~2 ms on AMD) instead of blocking the event loop for a full 16.7 ms refresh (legacy
  drivers fall back to the ioctl). Changes requested while on another VT now apply on return.
- **Resilient startup without Xwayland:** Xwayland crashing at startup no longer hangs session
  startup.
- **Per-output DRM error recovery, mode fallback, and DisplayPort retraining:** Failing outputs
  back off independently; modes exceeding link bandwidth fall back to lower refresh rates or
  resolutions; bad DisplayPort `link-status` outputs retrain automatically.
- **Screen capture optimizations and fixes:** Reused the DRM compositor's rendered frame on bare
  metal (unrotated monitors) and pooled offscreen textures; added presentation timestamps for OBS
  A/V sync; fixed stalled failed copies, off-screen window scale changes, invalid dmabuf error
  codes, and writing into destroyed client frames.
- **Adaptive sync fixes:** Advertised variable refresh rate in presentation feedback and restored
  VRR after monitor power cycling (`wlopm`/idle).
- **Documented secondary-GPU limitation:** Noted dual-GPU laptop outputs in the README and marked
  them in `pila-report`.
- **Fixed `pila-report` redaction:** Matched usernames on word boundaries so usernames like `dev`
  don't redact `/dev/dri/card1`.
- **Naming convention:** Standardized on "Pila" in prose (keeping `pila` for commands, packages,
  and paths).
- **CI and testing:** Added capture streaming, `--vrr`, gamma/capture frame budgets, and unknown
  flag rejection to `scripts/check`, plus a `vkms.yml` workflow testing DRM hotplug in CI.

## 0.4.0 (2026-11-08)

- **Moved to Smithay's `main` branch:** Switched from Smithay 0.7.0 (June 2025) to a pinned commit
  on Smithay `main` so upstream fixes land immediately.
- **Fixed Quickshell disconnects:** Destroying a layer/lock surface role before a null-buffer
  commit no longer triggers a protocol error.
- **Virtual keyboards (`wtype`, OSKs):** Virtual keyboard input now triggers bindings, counts as
  idle activity, and swaps in custom keymaps cleanly, restoring your physical keymap afterward.
- **Window menu:** Right-clicking any title bar (server-side or client-side) opens a window menu
  (minimize, maximize, always on top, all workspaces, close).
- **Urgency indicator:** Urgent windows (`xdg-activation` without serial, X11 `WM_HINTS`) show an
  amber title bar (`[colors] bar-urgent`) and flag their workspace until focused.
- **Dynamic title rules:** Window rules matching `title` now re-evaluate as window titles change.
- **Focus and activation fixes:** Clicking notifications/panels can activate apps; on-demand layer
  surfaces receive keyboard focus on map; stripped foreign compositor env vars (`SWAYSOCK`,
  `HYPRLAND_INSTANCE_SIGNATURE`, etc.) from child processes; logged protocol-error disconnects with
  PID and error.
- **Desktop shell verification:** Tested two Quickshell desktop shells feature by feature on
  headless Pila.
- **Drag-and-drop icons:** Rendered client drag icons following the pointer.
- **Surface lifecycle and X11 fixes:** Remapped layer surfaces and windows now show properly with
  decorations preserved; X11 apps can start minimized (`WM_HINTS`); X11 windows with
  `_GTK_FRAME_EXTENTS` measure geometry excluding client-side shadows.

## 0.3.1 (2026-11-07)

- **Clearer config error notifications:** Displayed `line N, column M: <reason>` instead of
  truncated parser headers, and noted `[bindings]` when bindings are placed outside it.
- **Improved `config.example.toml`:** Kept table headers active above commented examples so
  uncommenting lines lands in the right table; documented keyboard `model`, `[input.mouse]`,
  `scale` range, and `Mod` + drag.
- **Packaging fixes:** Stopped running EGL tests inside `pila-git`'s `PKGBUILD`; added `bash`
  dependency; recommended `wl-clipboard`; downgraded `swaylock` and `pciutils` to Suggests in
  `.deb`/`.rpm` (optional in AUR).
- **`pila-report` fixes:** Reported `wl-copy` availability and restricted username redaction to
  whole words.

Since 0.3.0, a tester confirmed Pila running from LightDM inside an Arch VM.

## 0.3.0 (2026-11-06)

- **Removed `disable-internal-display`:** Configs containing it are now invalid; use
  `enabled = false` inside the internal panel's `[[output]]` block instead (automatically
  re-enables if no other monitor remains).
- **Window shadows:** Enabled drop shadows by default (`[shadow]`, per-app `shadow` rule; skipped
  on maximized, fullscreen, and CSD-shadow windows).
- **Custom `[colors]`:** Live-reloaded colors for title bars, buttons, background, and snap
  previews.
- **Window and monitor management:** Added always-on-top (`Mod+T`), sticky all-workspaces
  (`Mod+P`), quarter-corner drag snapping and `snap-top-left`..`snap-bottom-right` actions, show
  desktop (`Mod+H`), and cross-monitor focus/move (`Mod+O`, `Mod+Shift+O`).
- **`pila msg` IPC:** Added CLI and Unix socket IPC (`docs/IPC.md`) for querying windows, outputs,
  and workspaces (text/JSON) and running actions (refused while locked).
- **Fixed:** `scripts/check` no longer counts resting outputs as missing refreshes while another
  monitor animates.

## 0.2.3 (2026-11-06)

- **Window background blur:** Added `ext-background-effect` and `[[window-rule]]` `blur` support
  behind translucent windows, following rounded corners, animations, and workspace slides.
- **Link opening (`xdg-utils`):** Added `xdg-utils` as a recommended package and reported
  `xdg-open` status in `pila-report`.
- **Notifications and logging:** Notified on invalid configs at startup too, increased error
  notification timeout from 3 s to 10 s, recommended `libnotify`, flushed session logs immediately
  for `tail -f`, and reliably reported the active GPU in `pila-report`.
- **Docs and tests:** Clarified that rule `opacity` only lowers alpha, and hardened timing-sensitive
  tests on slower machines.

## 0.2.2 (2026-11-06)

- **`.deb` and `.rpm` packages:** Attached Debian 13+/Ubuntu 24.04+ and Fedora packages to every
  release (verified headless on clean installs).
- **Nix flake:** Added `flake.nix` (`nix run github:igalmarino/pila`, NixOS session support, and
  `nix develop`), verified on NixOS via `nix run`.
- **`pila-report`:** Added diagnostic report generator redacting home dir, username, and hostname.
- **Fixes:** Reported missing `libEGL.so.1` cleanly instead of crashing, and fixed an intermittent
  test flake under load.

## 0.2.1 (2026-11-05)

- **New protocols:** `xdg-session-management` (restore window geometry/state across restarts),
  `xdg-dialog` (modal dialogs), `xdg-toplevel-tag`, `wp-fifo`, `wp-commit-timing`,
  `linux-drm-syncobj`, and implicit dmabuf render synchronization.
- **Windows and monitors:** Transient family stacking and workspace tracking; EDID make/model/serial
  matching in `[[output]]`; restoring monitors hotplugged while on another VT.
- **Security and reliability:** Filtered `ext-workspace`, `keyboard-shortcuts-inhibit`, and
  `ext-idle-notify` from sandboxes; bounded session storage; private log directory; Xwayland
  auto-restart (capped at 3/min) with clean signal mask; graceful output config failure; safe
  hotplug global retirement; isolated `scripts/tty` session state.
- **Blur and polish:** Fixed fractional-scale blur artifacts, scale changes, region captures, and
  overlapping blur; freed memory on disabled outputs; loaded cursor themes on a thread; made GPU
  capture waits non-blocking; added `scripts/layer-namespaces`.
- **Fixes:** Scoped fullscreen panel hiding to its own output; fixed pointer hits on popups over
  title bars; fixed closing a window mid-`Mod+Tab`; kept focus off X11 menus.

## 0.2.0 (2026-11-05)

- **Animations:** Added configurable `[animations]` for window open/close, minimize, maximize,
  fullscreen, snap, layer popups, and horizontal workspace slides.
- **Window and layer rules:** Added `[[window-rule]]` (`app-id`, `title`, wildcards) and
  `[[layer-rule]]` (`namespace`) for animations, workspace, fill state, geometry, title bar,
  corner radius, opacity, and blur.
- **Frame pacing and diagnostics:** Eliminated bare-metal animation stutter and first-animation
  hitches; added `RUST_LOG=pila::frames=debug` stats; added `pila --check-config`; added optional
  `corner-radius`; filtered privileged protocols from Flatpak sandboxes via `wp-security-context`.
- **Workspaces:** Configurable shared workspaces (`workspaces`, default 4) with `Mod+1..9`,
  `Mod+Page_Up/Down`, four-finger swipes, and `ext-workspace`.
- **Built-in display toggle:** Added `disable-internal-display = true` (replaced in 0.3.0 by
  `[[output]] enabled = false`).
- **Testing infrastructure:** Added `--backend headless`, automated integration suite in CI,
  `tests/FEATURES.md`, and `scripts/check`.
- **Fixes:** Cleared blocked signals for child processes; notified on invalid XKB layouts; throttled
  hidden surfaces; fixed null-buffer unmap/remap, closed-window capture cleanup, initial
  maximized/fullscreen sizes, exclusive layers over fullscreen, minimized window active state,
  pointer constraints with menus/focus loss, symlinked config reloads, D-Bus/gamma hangs, fontless
  startup, missing X11 socket dir, refreshless modes, and a 50 MiB session log cap.

## 0.1.0 (2026-11-04)

Initial public release: a stacking Wayland compositor for early testing.

- **Windows:** Server-side title bars and resize borders alongside client-side decorations;
  `Mod` + drag move/resize; maximize, fullscreen, half-screen snapping, keyboard move/resize/center,
  minimize, and `Mod+Tab` recent-use switching.
- **Monitors:** Multi-monitor hotplug, per-monitor mode, position, rotation, adaptive sync,
  fractional scaling, `wlr-output-management`, and `wlr-output-power-management`.
- **X11:** Xwayland support with bidirectional clipboard and primary selection sharing.
- **Desktop protocols:** Layer shell, session lock, idle notify/inhibit, `ext-data-control`,
  `ext-image-copy-capture`, `wlr-screencopy`, `ext-foreign-toplevel-list`,
  `wlr-foreign-toplevel-management`, `wlr-gamma-control`, `ext-background-effect`,
  `xdg-system-bell`, `xdg-activation`, `xdg-foreign`, pointer constraints/relative/gestures,
  `keyboard-shortcuts-inhibit`, virtual keyboard, viewporter, fractional scale, and presentation
  time.
- **Input and config:** Keyboard layout/repeat, libinput touchpad/mouse settings, three-finger
  swipes, customizable `[bindings]`, live-reloaded TOML config, login session wrapper
  (`scripts/install`), and Arch AUR packaging (`packaging/aur/pila-git`).
