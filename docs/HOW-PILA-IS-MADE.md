# How Pila is made

Most of Pila's code is written with an AI assistant, Claude Code. The README mentions this up
front, and every commit it worked on carries a `Co-Authored-By` trailer.

I know how that sounds in open source. Maintainers regularly get generated pull requests and bug
reports that were never tested and get dumped on them to review in their free time, and plenty of
projects now ban AI contributions because of it. A new project that admits it was built with AI is
easy to dismiss before anyone even opens the code.

I don't think AI assistance automatically leads to bad software. Unverified code does, and that was
already true long before AI. This page explains who makes the decisions in Pila, how changes are
verified, and what has gone wrong so far. You can check all of it directly against the repository.

## Why Pila

I started Pila because I wanted a classic floating desktop with overlapping windows that works well
with layer-shell desktop shells. Most layer-shell setups target tiling compositors, and the
stacking compositors I tried didn't fit what I wanted ([What sets Pila apart](WHY-PILA.md) has the
comparison).

From the start, I designed the project to rely on automated tests and measurements rather than
trusting anyone to catch every bug by reading code. What needs to be verified before a change is
trusted is decided up front, and the sections below explain how that works.

## Who decides

I do. Scope, technology choices, architecture, protocol support, the security model, performance
budgets, and testing standards are all my decisions, most of them written down in
[CLAUDE.md](../CLAUDE.md). I review what goes into the repository and take responsibility for it.

In practice, my review looks like this: I read the code that can't be verified solely from the
outside: input handling, session locking, sandbox protocol filtering, and the DRM/KMS backend. For
everything else, I read the tests first, because code passing a weak or wrong test proves nothing.
Visual changes and anything involving real hardware aren't committed until I've tested them myself.
If I don't like an implementation Claude Code wrote, I throw it out and have it redone.

## What the AI does

Claude Code writes most of the Rust code, tests, and documentation following my instructions. It
also translates what I write into English, including this page.

It runs inside a rootless dev container where Pila opens nested as a window on my desktop. Inside
the container, it can build the project, run the test suite, launch real Wayland clients against
Pila, and inspect logs. It can't see the screen or access bare-metal hardware, so anything visual
and everything in the DRM/KMS backend comes back to me to test.

The exact instructions it follows live in the repo: [CLAUDE.md](../CLAUDE.md),
[tests/CLAUDE.md](../tests/CLAUDE.md), and [scripts/CLAUDE.md](../scripts/CLAUDE.md). Human
contributors follow the same rules. They are written down in detail because an assistant follows
explicit rules well, but won't pick up unwritten conventions on its own.

## How changes are verified

Generated code can look clean and still be subtly broken, so I don't want correctness to depend on
anyone's gut feeling, mine included. Everything below is standard engineering discipline; when code
lands quickly, skipping any of it isn't an option.

### Integration tests

Every feature gets integration tests. Each test launches a real Pila instance on a headless
backend, connects real clients (a synthetic Wayland client, plus Xwayland for X11 tests), injects
input, and verifies state through a control socket. There are around 950 integration tests across
43 files and about 280 unit tests, all run in CI on every push.
[tests/FEATURES.md](../tests/FEATURES.md) maps every feature to its tests (or explains why it can't
be automated, such as bare-metal hardware or visual checks), and a test fails if that inventory ever falls
out of sync with the suite.

### Bugs

Every bug fix starts with a failing test that reproduces the problem, and that test stays in the
suite permanently. If testing uncovers a bug that requires a behavior decision, it stays checked in
as `#[ignore = "bug: ..."]` until I decide how it should behave.

### Invariants

Some bugs have no obvious visual symptom, so the test suite checks them continuously:

- **On-demand per-output redraws:** Under test, Pila panics if an output's rendered content changes
  without a corresponding redraw request (`PILA_HEADLESS_CHECK_REDRAWS`).
- **No stuck input, no lost windows:** Under test, Pila panics if a popup grab outlives its menu,
  if keyboard focus remains on a layer surface that doesn't accept keyboard input, if anything
  but the lock screen holds the keyboard or pointer while locked, if a window is in two places
  at once (say, on screen and minimized) or nowhere, or if a window ends up above one kept on
  top (`PILA_HEADLESS_CHECK_INVARIANTS`).
- **Session lock isolation:** While locked, any code path that tries to deliver input to anything
  other than a lock surface panics in debug builds.
- **No stale doc references:** A test fails if the documentation references a source file or symbol
  that no longer exists.
- **Backwards compatibility:** `tests/compat.rs` maintains a config file containing every key Pila
  has ever accepted, catching any change that would break existing user configs or `pila msg`
  scripts.

### Performance

Pila should never miss a monitor refresh while there is something new to draw, at any refresh rate,
and its average CPU time per frame should stay under 4 ms (a quarter of a 60 Hz frame).
`frame_stats.rs` logs frame counts, mean and worst CPU times, GPU times, missed refreshes per
output, and (on bare metal) end-to-end latency from redraw request to scanout. On bare metal, missed
refreshes are measured exactly from DRM vblank sequences.

`scripts/check` runs a release build through real programs (terminals, `xterm`, a launcher,
screenshots, screen sharing, blur, a fullscreen app, and a bar) and fails on any missed refresh or
budget violation. Changes touching rendering, input, or the event loop go through it before being
committed.

Latest bare-metal numbers (2026-11-17, 0.6.1 codebase, AMD Renoir laptop):
- **With an external monitor, on AC power:** 0 missed or late refreshes across 6,350 frames; peak
  5-second average CPU time of 1.23 ms/frame (3.6 ms worst single frame); GPU time at most 1.6 ms
  average and 4.8 ms worst.
- **Laptop screen only, on battery** (where everything runs roughly twice as slow): 0 missed
  refreshes across 5,246 frames; peak average CPU time of 2.0 ms/frame (7.6 ms worst single frame).
- **Exiting direct scanout:** dropped from 11.7 ms (on 2026-11-12) to 0.06 ms (see
  *Late frames on real hardware* below).

### Real hardware

Because the DRM/KMS backend can't run in the dev container, I run `scripts/check` on a spare TTY of
my development laptop, which also runs Pila as my daily login session. Testers have confirmed Pila
works on Intel GPUs, Debian with SDDM, NixOS, LightDM, and one NVIDIA GTX 1060. The README tracks
what still needs testing: newer NVIDIA cards, non-systemd systems, and multi-GPU outputs.

### CI

Every push runs `cargo fmt`, `clippy -D warnings`, a `--locked` release build, a staged install,
and `shellcheck`. Weekly, and whenever dependencies change, CI also builds on Debian, Ubuntu, Fedora,
and Nix, checks the minimum supported Rust version, and runs `cargo audit`. There are `cargo-fuzz`
targets for the config parser, EDID, gamma ramps, IPC requests, and Wayland protocol streams, plus
an experimental workflow running the DRM/KMS backend on the kernel's `vkms` virtual display driver.

### Documentation

[docs/ARCHITECTURE.md](ARCHITECTURE.md) documents each subsystem's flows and invariants, and
changes update the relevant section in the same commit. I rely on it just as much as the AI does:
nobody holds 50,000+ lines of code in their head.

## About the speed

The first commit was on 2026-10-31. Less than two weeks later, Pila was a daily-driver alpha with
roughly 36,000 lines of Rust and 20,000 lines of tests. Nobody writes that by hand in two weeks,
and I'm not pretending I did.

Speed only helps if verification keeps up with it. Every change still has to pass the test suite,
stay within the frame budget, and (when it touches rendering) be tested on real hardware.

As I understand the term, "vibe coding" means accepting AI output because it looks like it works.
I've laid out how changes get into Pila above. I don't think the label fits, though you're better
off judging that from the repository than from my word.

## What went wrong

The AI makes mistakes, and so do I. All of these are in the git history.

### A client could crash Pila

Region screen capture used client-supplied coordinates without bounds checking: negative sizes
tripped an assertion in Smithay, and values near `i32::MAX` could overflow. Auditing every
client-supplied number caught the issue. Invalid regions now fail the capture cleanly and are
covered by tests. A follow-up pass removed every unchecked `unwrap` and `expect` past startup
unless backed by an explicit local invariant, since a panic in a compositor brings down the entire
session.

### A missing library caused a panic

A NixOS tester (issue #1) hit a panic when `libEGL` couldn't be loaded. Everything worked on my
laptop and in CI, so it took someone on another distro to find it. Pila now checks for `libEGL` at
startup, prints which library is missing and how to fix it, and has a test that runs without the
library present.

### Docs drifted from the code

After a few days of rapid development, a doc review caught the manual and man page describing
outdated behavior and comments pointing to removed functions. Fixing them manually once wouldn't
hold, so we added tests that fail whenever docs reference nonexistent code symbols or omit default
keybindings.

### Manual review caught gaps in test coverage

An early review of animations and window rules turned up bugs that no test caught because nothing
was checking for them: closing windows fading out over the newly switched workspace, X11 windows
maximized by a rule opening without animation, and rules running twice per window. We added
scenarios to `scripts/check` and verified the fixes both nested and on bare metal.

### Late frames on real hardware

While updating the benchmark numbers for this page, three consecutive `scripts/check` runs dropped
one frame each out of roughly 1,900, even though the previous day had been clean. None of this
reproduced in the container because it has no real vblank. I enabled per-frame logging and re-ran
both with two monitors and with the external monitor unplugged. My initial guess was wrong; the
traces pointed to three separate issues:

- **Cold driver setup:** The first window of a session triggered one-time GPU driver setup during
  its first frame, doubling frame time (2.6 ms vs. 1.3 ms). Warming up the draw paths at startup
  brought that frame down to 1.4 ms.
- **Direct scanout format mismatch:** Leaving fullscreen direct scanout stalled Pila's event loop
  for 11–14 ms with two monitors (vs. 0.3 ms with one). Smithay placed the client buffer on the
  primary plane as opaque `XRGB8888`, while Pila rendered its own frames as `ARGB8888`. Switching
  pixel formats made `amdgpu` treat it as a plane reset and acquire a global lock waiting for every
  monitor's pending page flip. Switching Pila's framebuffers to `XRGB8888` brought the dual-monitor
  transition down to 0.06 ms.
- **Battery power:** Unplugging the external monitor also unplugged USB-C charging. On battery,
  frame costs roughly doubled and the heaviest frame (opening a window) missed its deadline. That
  wasn't a bug in Pila, but it clarified that the 4 ms budget applies on AC power.

After fixing the first two, the dual-monitor run came back completely clean (see the numbers
above). Whenever possible, each fix came with a regression check; bare-metal timing can only be
verified by running `scripts/check` on real hardware.

### Late rendering

Version 0.6.1 introduced late rendering: drawing each frame as close to the monitor refresh as
safely possible rather than right after the previous vblank, so recent client commits still make
that refresh. On my laptop, redraw-to-scanout latency dropped from 25.8 ms to 21.3 ms on the
internal screen and from 28.5 ms to 21.7 ms on the external monitor. Getting there took about
fifteen TTY runs of `scripts/check`, half of which failed, and what shipped was not the first
version that passed:

- The first version computed its safety margin from each monitor's recent worst frame plus the
  other monitor's worst. That passed with two monitors, but trimming the other monitor's part to
  save 1–2 ms caused late frames in two out of three runs whenever an app started up on the same
  GPU, which Pila's own frame history couldn't predict.
- On the laptop screen alone on battery, even the original margin failed: the second monitor's
  margin had been masking GPU contention from apps launching, maximizing, closing, or animating.
- The real fix was behavioral rather than a magic constant: only hold frames when nothing on screen
  is opening, closing, resizing, or animating; never shrink the margin below half a refresh; and
  release a held frame immediately if the scene stops being steady. The final bug was a frame held
  just before a window opened that stayed delayed even after the new window entered it.
- Along the way, we found an unrelated bug: blurred windows discarded their cached backdrop when
  switching workspaces and rebuilt it upon returning, which on battery was enough to miss a
  refresh.

Every piece that could be unit- or integration-tested got a test; timing itself still relies on
`scripts/check`. Frame statistics now log GPU time and per-frame latency, and `pila-check` prints
them so testers can see whether the margin holds on their hardware.

## Other projects' rules

Pila accepts AI-assisted contributions as long as the pull request says so and the author
understands and stands behind the code. Other projects set different policies, and I respect them:
Smithay doesn't allow AI-written text in issues or pull requests, so anything I post there I write
by hand.

## Check it yourself

```bash
cargo test
scripts/check --nested    # or scripts/check from a TTY on real hardware
scripts/coverage
```

If something doesn't hold up, open an issue, regardless of who or what wrote the code.
