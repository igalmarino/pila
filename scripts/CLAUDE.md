# scripts/check

`scripts/check` drives a real Pila with real programs (a terminal, xterm, fuzzel, grim,
waybar if installed: windows opening and closing, minimize, maximize, workspace switches, the
overview opening and closing, a
launcher, a capture, a capture stream, a blurred window, a fullscreen one, and a fullscreen foot
when the terminal was kitty: shm buffers, copied to the GPU at each commit), over a wallpaper
(swaybg on a 4K gradient it writes with python3, if both are installed), and judges the log: the scenarios finished without a panic, animations
ran (`starts` on `pila::animation`), swaybg's layer surface came, waybar's surfaces follow their layer rule, grim got a
picture, the stream got frames, monitors were identified from their EDID (bare metal), and the
frame statistics (no missed refresh, frames shown late included, CPU per frame on average within 4 ms, for the outputs'
frames and, on average over all of them, the captures'; each copy of a client's shm buffer at
its commit, `uploads`, and each capture's copy into shm once the GPU is done, `copies`, within
8 ms (half a 60 Hz period, render late's margin), since they hold the event loop; bare metal, if a night light set gamma,
its changes within that budget too: commits that don't wait). It prints PASS/FAIL per check
and exits 1 on a failure (`scripts/check --judge <log>` judges a log again; `PILA_BIN` runs
another binary; installed as `pila-check` by `scripts/install` and the packages, it runs the
`pila` next to it on bare metal without `scripts/tty`, its helpers come from
`share/pila/check` and its logs go to `~/.local/state/pila`, all decided by whether it sits in
a `scripts` directory; `PILA_CHECK_RUST_LOG` adds `RUST_LOG` directives to the run's own, such as
`smithay::backend::drm::compositor=trace` for why direct scanout starts and stops); INFO lines report without judging (how long gamma set the blocking
way took: after a modeset, or a driver without `GAMMA_LUT`; the first frames after outputs were
set up, which the budget leaves out; whether monitors on another GPU had their frames copied
through memory; the redraws per output that found nothing new, `idle` in
the frame statistics, at most one per refresh on bare metal; the GPU's time per frame per
output, `gpu_avg_ms` and `gpu_max_ms` (where the renderer gives native fences); on bare metal
the frames' latency per output, how many were held to render late, how far before their vblank
and how long before it they were ready (`PILA_CHECK_RENDER_LATE=false`
runs without it, to compare); the event loop's own work after each iteration, `loop`,
how often it did all of its housekeeping and the CPU it took; on bare metal, how many frames
were an app's buffer as it is, `direct`: direct scanout, which the fullscreen terminal may
bring; and how many were torn, `torn`, by `vkcube` with vsync off, fullscreen, where it's installed). `--vrr` (bare metal) asks every
monitor for adaptive sync and checks it comes on where a monitor can do it, that Pila then
reports a variable refresh (`refresh reported to clients` on `pila::frames`), and that it's
still on after `wlopm` powers the monitors off and on; whether the monitor's refresh follows
the content is still for the eyes.

It doesn't test behavior: that's the integration tests' job (`scripts/toplevel-action`,
`scripts/workspace-action` and `scripts/capture-stream` are its small taskbar, pager and screen
share clients, standard-library Python).

A feature adds a scenario there (a `mark "name begin/end"` section in the workload, checks
with `section`, `has`, `lacks` on a debug log target) only if it's bare-metal only or puts load
on rendering; what only eyes can judge (how it looks) is still asked of the user.

# scripts/apps

`scripts/apps [gtk4|foot|kitty|weston-terminal|xterm ...]` runs real apps, one at a time, each
on a sealed headless Pila (a temporary HOME and runtime directory and a private D-Bus session bus,
so it never touches the session it's run from; one 1920x1080 output, with
`PILA_HEADLESS_CHECK_INVARIANTS`), and drives their windows with real input through the control
socket: a drag by the title bar (Pila's, or the app's own) and Mod+drag move them exactly as
far as the pointer, a click on the title bar moves nothing, and maximized or snapped, a drag
by the title bar brings them back to their size under the pointer; after each step no grab is
left, the keyboard is on the window and no popup is open. It judges Pila's log (no
panic, no client disconnected for a protocol error) and prints PASS/FAIL. Results are in `pila-apps/<app>/`. It's for what
the synthetic test client can't show, since real apps do things it doesn't (GTK 4 asks to leave
fullscreen as a drag on its own title bar starts; a bug like that is then reproduced in a test,
with the GTK-like client of `tests/common/client.rs` where it helps). Run it after changing
grabs, decorations, fill states or input, and before a release; a behavior that only real apps
show gets a step. Standard-library Python, like the other helpers.

`switcher` drives the window switcher Pila draws (`run_switcher`) with three foot windows
(`--app-id switch-a` ...), reading it from the state (`switcher_view`): a quick Super+Tab, there
and back, shows nothing; Super held with a tap of Tab shows it, centered on the output, a tile per
window in a row, the window switched to keeping the keyboard (`switcher.png`: real apps' live
previews, shrunk whole); another Tab steps on with it shown; letting go of Super takes it
away, on the window switched to; and with Alt+Tab held (Alt isn't Mod there), a click on the last
tile switches to the least recent window and takes it away, with no grab left. Run it after
changing the switcher, `switcher.rs` or `switcher_view.rs`.

`overview` drives the overview Pila draws (`run_overview`) with two foot windows and GTK 4's
widget factory (if installed), animations on, reading it from the state (`overview`): Super+w
opens it, every window in a place of its own inside the output, none over another, under the
strip, a thumbnail per workspace (`overview.png`: real apps in their places); a click on one picks
it, closed with that window focused and holding the keyboard, no grab left; opened again, a window
dragged onto workspace 2's thumbnail goes there with the overview still open and no grab left, and
Escape closes it; the pointer pushed into the top-left corner opens it and, back again, closes it.
Run it after changing `overview.rs` or the input paths it takes.

`fcitx5-foot` and `fcitx5-gtk4` type through a real input method instead (`run_ime`; skipped
where fcitx5 or Rime's `luna_pinyin` isn't installed): fcitx5 starts in the sealed HOME with Rime
as its input method, on from the start, and that one schema (deploying all of Rime's takes
minutes; it's ready when Rime's `user.yaml` has a `last_build_time`), and blur on (in its settings
and in a copy of its default theme, `blurred`); the app is foot writing
what's typed to a file, or GTK Demo's text field, read back through the clipboard. The checks:
fcitx5 becomes Pila's input method and takes the keyboard in the app, `nihao` shows the
candidate window next to the text cursor and on the screen (GTK's text cursor has no width: the
check that Pila's Smithay keeps such a rectangle) with a blurred backdrop, as fcitx5 asks through
ext-background-effect (the theme is opaque: `blur.entries` in Pila's state tells), a space puts 你好 into the app, Super+m still
maximizes, and in GTK a context menu opens and Escape closes it with no grab left. The container
has CJK fonts (wqy-microhei, noto-fonts-cjk): `candidates.png` shows the candidates readable. Run them after changing
`input_method.rs`, the keyboard path, grabs, popups' drawing or blur, or the Smithay rev.

# scripts/demo-video

`scripts/demo-video` makes a demo video on a sealed headless Pila (as `scripts/apps`: one
1920x1080 output at scale 1.5, default looks: `swaybg`, `waybar` and `mako` started through
Pila (the control socket's `spawn`) as its autostart starts them, so with its configuration
for them (`tool-configs`) and its wallpaper, `fuzzel` as the launcher, UTC):
`scripts/demo-scenes` plays captioned scenes with real input (pointer and Pila's bindings
through the control socket, text through `wtype`), waiting on Pila's state, never on fixed
sleeps for something to happen; `scripts/demo-record` records the output at a constant 60 fps
(an ext-image-copy-capture client: each frame in the tick of its presentation time, ticks with
nothing new repeated) into a lossless `pila-demo/master.mkv`, and ffmpeg captions it into
`pila-demo.mp4` (every scene, 1080p60), `pila-demo-short.mp4` (the scenes marked short, 720p,
under 10 MB) and the README's `docs/demo.webp`: the short one as an animated WebP (1280 px,
30 fps), because GitHub's README shows a video still, with controls, never playing by itself
or looping (and a paragraph that is only a link to an attached video becomes its player, so
the image stands alone). The WebP comes from libwebp's animation encoder through `ctypes`
(`encode_webp`), not ffmpeg's `libwebp_anim`: libwebp 1.6 skips blocks that changed less than a
tolerance taken from `quality` since the previous *source* frame, so slow fades left stale
blocks on the picture; `quality` stays 100 and a target PSNR sets the compression (with sharp
YUV, for colored text). It's committed (keep it under 5 MB, GitHub's image proxy refuses more;
the script warns); the mp4 files aren't, they're uploaded as GitHub attachments, which stay
private until published content (release notes) links them: check a link logged out.
`--shots` records nothing and takes a screenshot per scene (`pila-demo/shots/`), the quick way
to check scenes; `--encode` encodes the last recording again. It fails when a scene's wait
times out, a saved config doesn't load or Pila panics, and lists missed refreshes from
`pila::frames`. Redo it when the looks, the bar, the launcher or the scenes change, and watch
it before publishing it: no personal data. A scene that needs a new kind of input or wait gets
a helper in `demo-scenes`, not a sleep.

# scripts/install

`scripts/install` builds the release binary (with the host's cargo if there is one, after
checking what the build needs from the system, `check_build_deps`: keep its list in step with
`docs/INSTALL.md`'s; else `--container`; `--no-build`) and installs it with the files listed in its `install_files`
(what they are: `docs/INSTALL.md`), among them `resources/session/` (the `pila-session`
wrapper, the `wayland-sessions` entry and, where there's systemd, the user units Pila runs
as); `scripts/uninstall` removes them. Paths come from `scripts/install-paths` (`PREFIX`,
`BINDIR`, `DATADIR`, `SYSCONFDIR`, `DESTDIR`), sourced by both.
