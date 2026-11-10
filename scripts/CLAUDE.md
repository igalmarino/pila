# scripts/check

`scripts/check` drives a real Pila with real programs (a terminal, xterm, fuzzel, grim,
Noctalia if installed: windows opening and closing, minimize, maximize, workspace switches, a
launcher, a capture, a capture stream, a blurred window) and judges the log: the scenarios finished without a panic, animations
ran (`starts` on `pila::animation`), Noctalia's surfaces follow their layer rule, grim got a
picture, the stream got frames, monitors were identified from their EDID (bare metal), and the
frame statistics (no missed refresh, CPU per frame on average within 4 ms, for the outputs'
frames and, on average over all of them, the captures'; bare metal, if a night light set gamma,
its changes within that budget too: commits that don't wait). It prints PASS/FAIL per check
and exits 1 on a failure; INFO lines report without judging (how long gamma set the blocking
way took: after a modeset, or a driver without `GAMMA_LUT`; the first frames after outputs were
set up, which the budget leaves out; the redraws per output that found nothing new, `idle` in
the frame statistics, which nothing paces on bare metal). `--vrr` (bare metal) asks every
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
bugs of a shell. Results, screenshots included, are in `pila-shells/<shell>/`: look at them.
Each run is sealed off (temporary HOME and runtime dir, private D-Bus) so it never touches the
session it's run from. Run it after changing a protocol a shell uses, after a Smithay bump and
before a release; a new shell feature that depends on Pila gets a step. Pitfalls: Quickshell's
IPC exits 0 on a bad call (`step` reads the answer), window bindings act on the window with the
keyboard (wait for a closing panel to give it back), and a shell animates its own panels away.
