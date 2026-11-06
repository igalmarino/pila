# scripts/check

`scripts/check` drives a real pila with real programs (a terminal, xterm, fuzzel, grim,
Noctalia if installed: windows opening and closing, minimize, maximize, workspace switches, a
launcher, a capture) and judges the log: the scenarios finished without a panic, animations
ran (`starts` on `pila::animation`), Noctalia's surfaces follow their layer rule, grim got a
picture, monitors were identified from their EDID (bare metal), and the frame statistics (no
missed refresh, CPU per frame on average within 4 ms). It prints PASS/FAIL per check and exits
1 on a failure.

It doesn't test behavior: that's the integration tests' job (`scripts/toplevel-action` and
`scripts/workspace-action` are its small taskbar and pager clients, standard-library Python).

A feature adds a scenario there (a `mark "name begin/end"` section in the workload, checks
with `section`, `has`, `lacks` on a debug log target) only if it's bare-metal only or puts load
on rendering; what only eyes can judge (how it looks) is still asked of the user.
