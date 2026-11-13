# scripts/check

`scripts/check` drives a real Pila with real programs (a terminal, xterm, fuzzel, grim,
Noctalia if installed: windows opening and closing, minimize, maximize, workspace switches, a
launcher, a capture, a capture stream, a blurred window, a fullscreen one) and judges the log: the scenarios finished without a panic, animations
ran (`starts` on `pila::animation`), Noctalia's surfaces follow their layer rule, grim got a
picture, the stream got frames, monitors were identified from their EDID (bare metal), and the
frame statistics (no missed refresh, frames shown late included, CPU per frame on average within 4 ms, for the outputs'
frames and, on average over all of them, the captures'; bare metal, if a night light set gamma,
its changes within that budget too: commits that don't wait). It prints PASS/FAIL per check
and exits 1 on a failure (`scripts/check --judge <log>` judges a log again; `PILA_BIN` runs
another binary; installed as `pila-check` by `scripts/install` and the packages, it runs the
`pila` next to it on bare metal without `scripts/tty`, its helpers come from
`share/pila/check` and its logs go to `~/.local/state/pila`, all decided by whether it sits in
a `scripts` directory; `PILA_CHECK_RUST_LOG` adds `RUST_LOG` directives to the run's own, such as
`smithay::backend::drm::compositor=trace` for why direct scanout starts and stops); INFO lines report without judging (how long gamma set the blocking
way took: after a modeset, or a driver without `GAMMA_LUT`; the first frames after outputs were
set up, which the budget leaves out; the redraws per output that found nothing new, `idle` in
the frame statistics, at most one per refresh on bare metal; the event loop's own work after each iteration, `loop`,
how often it did all of its housekeeping and the CPU it took; on bare metal, how many frames
were an app's buffer as it is, `direct`: direct scanout, which the fullscreen terminal may
bring). `--vrr` (bare metal) asks every
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

# scripts/shells

`scripts/shells [noctalia|dms]` runs each desktop shell Pila supports (Noctalia, Dux Shell) on
a headless Pila with two outputs (one at scale 1.5, named like monitors: Dux Shell's night mode
leaves out `HEADLESS-*` outputs) and walks its own IPC through every feature that depends on
the compositor, checking the shell's answers and Pila's state (the control socket) after each
step: panels, launcher and keyboard, workspaces, taskbar, notifications, clipboard,
screenshots, gamma, output management and power, the lock screen. It judges Pila's log (no
panic, no client disconnected for a protocol error) and prints PASS/FAIL, plus NOTE for known
bugs of a shell (`scripts/shells --judge <dir>` judges an earlier run again). Results,
screenshots included, are in `pila-shells/<shell>/`: look at them.
Each run is sealed off (temporary HOME and runtime dir, private D-Bus) so it never touches the
session it's run from. Run it after changing a protocol a shell uses, after a Smithay bump and
before a release; a new shell feature that depends on Pila gets a step. Pitfalls: Quickshell's
IPC exits 0 on a bad call (`step` reads the answer), window bindings act on the window with the
keyboard (wait for a closing panel to give it back), and a shell animates its own panels away.

# scripts/screenshot

`scripts/screenshot` makes `docs/screenshot.png` (a still for listings and posts; the README
shows `docs/demo.webp` instead) on a sealed headless Pila (as
`scripts/shells`), with its default looks: Noctalia's bar on a generated gradient (its first-run
dialog skipped by its `.setup-complete` marker), GTK Demo with its own title bar, and two kitty
windows placed by window rules, one translucent with blur over GTK Demo. Redo it when the looks
or the shells change, and look at it before committing it: no personal data (a sample config,
no prompt with a user or host name).

# scripts/demo-video

`scripts/demo-video` makes a demo video on a sealed headless Pila (as `scripts/screenshot`: one
1920x1080 output at scale 1.5, default looks, Noctalia's bar, UTC): `scripts/demo-scenes` plays
captioned scenes with real input (pointer and Pila's bindings through the control socket, text
through `wtype`), waiting on Pila's state, never on fixed sleeps for something to happen;
`scripts/demo-record` records the output at a constant 60 fps (an ext-image-copy-capture client:
each frame in the tick of its presentation time, ticks with nothing new repeated) into a lossless
`pila-demo/master.mkv`, and ffmpeg captions it into `pila-demo.mp4` (every scene, 1080p60),
`pila-demo-short.mp4` (the scenes marked short, 720p, under 10 MB) and the README's
`docs/demo.webp`: the short one as an animated WebP (1280 px, 30 fps), because GitHub's README
shows a video still, with controls, never playing by itself or looping. The WebP comes from
libwebp's animation encoder through `ctypes` (`encode_webp`), not ffmpeg's `libwebp_anim`:
libwebp 1.6 skips blocks that changed less than a tolerance taken from `quality` since the
previous *source* frame, so slow fades left stale blocks on the picture; `quality` stays 100 and
a target PSNR sets the compression (with sharp YUV, for colored text). It's committed (keep it under 5 MB, GitHub's image proxy
refuses more; the script warns); the mp4 files aren't, they're
uploaded as GitHub attachments, which stay private until published content (release notes)
links them: check a link logged out. `--shots` records nothing and takes a
screenshot per scene (`pila-demo/shots/`), the quick way to check scenes; `--encode` encodes the
last recording again. It fails when a scene's wait times out, a saved config doesn't load or Pila
panics, and lists missed refreshes from `pila::frames`. Redo it when the looks, the shells or
the scenes change, and watch it before publishing it: no personal data. A scene that needs a
new kind of input or wait gets a helper in `demo-scenes`, not a sleep.
