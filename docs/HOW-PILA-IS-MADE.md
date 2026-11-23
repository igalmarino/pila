# How Pila is made

Most of Pila's code is written by an AI assistant, Claude Code. The README says so, and every
commit it worked on has a `Co-Authored-By` trailer.

I know how that sounds to many people in Open Source. Maintainers get pull requests and issues
that were generated, never tested, and left for them to review in their free time, and some
projects now have rules against it. A new project that says "made with AI" tends to get
dismissed before anyone opens the code.

I don't think AI assistance has to mean bad software. Unverified code is the problem, and it
was a problem before AI. This page explains who decides what in Pila, how changes are verified
and what has gone wrong so far. You can check all of it against the repository.

## Why Pila

I started Pila because I wanted a classic desktop with overlapping windows for layer-shell
desktop shells. They usually run on tiling compositors, and the stacking compositors I tried
didn't work with them the way I wanted ([What sets Pila apart](WHY-PILA.md) has the comparison).

The project depends on tests and measurements more than on anyone reading every line: what has
to be tested before it's trusted is decided up front, and the sections below say how.

## Who decides

I do. Scope, technology, architecture, which protocols Pila supports, the security model, the
performance budget and what counts as tested are my decisions, and most of them are written
down in [CLAUDE.md](../CLAUDE.md). I review what goes in and I answer for it.

Reviewing, for me, means this. I read the code that can't be checked from outside: input
handling, the session lock, what sandboxed clients can see, the DRM/KMS backend. For everything
else I read the tests first, since code that passes a wrong test proves nothing. Visual changes
and anything that depends on real hardware aren't committed until I've seen them work. When I
don't like what Claude Code wrote, I discard it and it gets implemented again.

## What the AI does

Claude Code writes most of the code, the tests and the documentation from my instructions. It
also translates what I write into English, and this page went through it too.

It runs in a dev container without root, where Pila runs nested as a window on the host desktop.
There it can build, run the tests, start real clients against Pila and read the logs. It can't
see the screen and can't touch the real hardware, so anything visual and everything in the
DRM/KMS backend comes back to me.

The instructions it works under are in the repository: [CLAUDE.md](../CLAUDE.md),
[tests/CLAUDE.md](../tests/CLAUDE.md) and [scripts/CLAUDE.md](../scripts/CLAUDE.md). A human
contributor is held to the same rules. They're written in that much detail because an assistant
does what's written and doesn't pick up conventions on its own.

## How changes are verified

Generated code can look right and be wrong, so I don't want correctness to rest on anyone's
impression of the code, mine included. What follows is ordinary engineering practice. With code
arriving this fast, I can't afford to skip any of it.

### Integration tests

Every feature has them. A test starts a real Pila on a headless backend, connects real Wayland
clients (a synthetic one, and Xwayland for X11), injects input and checks the result through a
control socket. There are about 950 integration tests in 43 files and about 280 unit tests, and
CI runs them on every push. [tests/FEATURES.md](../tests/FEATURES.md) lists every feature with
its tests, or the reason it has none (it needs bare metal, or only eyes can judge it). A test
fails if that list and the suite disagree.

### Bugs

A bug fix starts with a test that reproduces the bug and fails. The test stays in the suite. If
a test turns up a bug that needs a decision about behavior, it stays as `#[ignore = "bug: ..."]`
until I decide.

### Invariants

Some things break without any visible sign, so the suite checks them:

- Pila redraws on demand, per output. Under test it panics if an output shows something new
  that no redraw request accounts for (`PILA_HEADLESS_CHECK_REDRAWS`).
- Input can't get stuck: under test Pila panics if a menu's grab outlives the menu, or the
  keyboard stays on a panel that takes none (`PILA_HEADLESS_CHECK_INVARIANTS`).
- While the session is locked, a code path that would give input to anything but the lock
  screen panics in debug builds.
- A test fails if the docs name a file or a function that no longer exists.
- `tests/compat.rs` keeps a config file with every key Pila has ever accepted, so a change that
  would break someone's config or `pila msg` script fails.

### Performance

Pila shouldn't miss a refresh while there's something to show, at whatever rate the monitor
runs, and its CPU time per frame has to average under 4 ms (a quarter of a 60 Hz period).
`frame_stats.rs` logs frames, average and worst CPU time, the GPU's time and missed refreshes
per output and, on bare metal, how long each frame took from being asked for to being on screen.
On bare metal the missed refreshes are exact, taken from the DRM vblank sequence. `scripts/check`
runs a release build through real programs (terminals, xterm, a launcher, screenshots, a screen
share, blur, a fullscreen app, a bar) and fails on a missed refresh or a blown budget.
Changes to rendering, input or the event loop go through it before they're committed.

Last bare-metal runs, 2026-11-17, with 0.6.1's code, on an AMD Renoir laptop. With an external
monitor, on mains power: no missed or late refresh in 6,350 frames, at most 1.23 ms of CPU per
frame on average over five seconds, 3.6 ms for the slowest frame, and the GPU's part at most
1.6 ms on average and 4.8 ms at worst. With the laptop's screen alone, on battery, where
everything costs about twice as much: none missed in 5,246 frames, at most 2.0 ms on average
and 7.6 ms for the slowest frame. Leaving direct scanout, which took 11.7 ms on 2026-11-12, takes
0.06 ms (see "Late frames on real hardware" below).

### Real hardware

The DRM/KMS backend can't run in the container, so I run `scripts/check` on a spare TTY of the
development laptop, which also runs Pila as its login session. Testers have reported it working on Intel GPUs, Debian with SDDM, NixOS,
LightDM and one NVIDIA card (a GTX 1060). The README lists what nobody has tried yet: newer
NVIDIA cards, systems without systemd, monitors on a second GPU.

### CI

Every push runs `cargo fmt`, `clippy -D warnings`, a `--locked` build, a staged install and
`shellcheck`. Once a week, and whenever dependencies change, it builds on Debian, Ubuntu, Fedora
and Nix and with the minimum supported Rust, and runs `cargo audit`. There are fuzz targets for
the config parser, EDID, gamma ramps, IPC requests and the Wayland protocol, and an experimental
job that runs the DRM/KMS backend on vkms, the kernel's virtual KMS driver.

### Documentation

[docs/ARCHITECTURE.md](ARCHITECTURE.md) has a section per subsystem with its flows and
invariants. A change to a flow updates its section in the same commit. I need it as much as the
AI does: nobody keeps more than 50,000 lines in their head.

## About the speed

The first commit is from 2026-10-31. Less than two weeks later Pila was a daily-driver alpha
with about 36,000 lines of Rust and 20,000 of tests. Nobody writes that by hand in two weeks,
and I'm not going to pretend I did.

That speed is only useful if checking keeps up with it. Whatever goes in still has to pass the
tests, stay within the frame budget and, when it touches rendering, run on real hardware first.

"Vibe coding", as I understand the term, means accepting what the AI writes because it seems to
work. I've described above how changes get into Pila. I don't think the term fits, but you can
judge that from the repository better than from this page.

## What went wrong

The AI gets things wrong. So do I. These cases are all in the history.

### A client could crash Pila

A screen capture of a region used the numbers the client sent without checking them. A negative
size tripped an assertion in Smithay, and values near `i32::MAX` could overflow. A review of
every number a client sends found it. An invalid region now fails the capture, and tests send
those values. A later pass removed every `unwrap` and `expect` past startup, except where the
code states the invariant it relies on. A panic in a compositor takes the whole session with it.

### A missing library was a panic

A tester on NixOS (issue #1) got a panic when `libEGL` couldn't be loaded. It worked on the
development machine and in CI, so it took a user on another distribution to find it. Pila now checks at
startup and says which library is missing and what to do about it. A test runs it without the
library.

### The docs drifted from the code

After a few days of fast changes, a review of the docs found the manual and the man page
describing behavior that had changed, and comments naming functions that were gone. Fixing them
once wouldn't have lasted, so there's now a test that fails when the docs name code that doesn't
exist, and another that fails when the manual is missing a default binding.

### A review found what the tests didn't cover

An early review of the animation and window-rule code found bugs that no test caught, because no
test looked there: closing windows fading out over the next workspace after a switch, an X11
window maximized by a rule opening with no animation, rules evaluated twice per window.
`scripts/check` got scenarios for them, and the fix was checked nested and on real hardware before
it went in.

### Late frames on real hardware

When I went to update the numbers on this page, three `scripts/check` runs in a row had one late
frame each, out of about 1,900. The previous day's run had been clean. None of this shows up in
the container, which has no real vblank. I turned on per-frame logging and ran it again, once
with both monitors and once with the external one unplugged. The first guess at the cause was
wrong. The traces showed three different things:

- The first window of a session paid for a one-time setup in the GPU driver in its first frame,
  which took twice as long as later ones (2.6 ms against 1.3 ms). Pila now does that at startup
  and the frame takes 1.4 ms.
- When a fullscreen app leaves direct scanout, the kernel waits for the other monitor's pending
  frame before applying the change: 11 to 14 ms with Pila's event loop blocked, against 0.3 ms
  with a single monitor. Smithay puts a client's buffer on the primary plane as opaque
  (XRGB8888), while Pila's own frames were ARGB8888, so every switch changed the framebuffer's
  format. amdgpu treats that as a plane reset and takes a global lock that waits for every
  monitor's pending flip. Pila's frames are XRGB8888 now. With both monitors the switch takes
  0.06 ms.
- The single-monitor run was on battery, because the monitor's cable also charges the laptop.
  Everything cost about twice as much, and the most expensive frame (opening a window) no longer
  fit in its margin. That's not a bug in Pila, but it does mean the 4 ms budget is a mains-power
  number.

The first two are fixed, and the third is a condition of the measurement rather than a cause.
The run after both fixes, with both monitors, was clean (see the numbers above).

Where a check was possible, each of these fixes added one, so the same mistake fails the suite
next time. The late frames are the exception: that's timing on real hardware, and only
`scripts/check` measures it.

### Rendering late

0.6.1 draws each frame as close to the monitor's refresh as it safely can, instead of right after
the last one, so what apps draw in the meantime still makes that refresh. On the development laptop the time
from a redraw being asked for to the frame on screen went from 25.8 to 21.3 ms on its own screen
and from 28.5 to 21.7 ms on the external monitor. It took about fifteen `scripts/check` runs on
the TTY, half of them failed, and what shipped isn't the first version that passed:

- The first version took its margin from the worst of each monitor's recent frames, plus the
  other monitor's worst. It passed with both monitors. A leaner margin without the other
  monitor's part saved one or two more milliseconds and made a frame late in two runs of three:
  an app starting up on the same GPU, which nothing in Pila's own frames predicts.
- With the laptop's screen alone, on battery, the original margin failed too: the other
  monitor's part had been covering for it. Each run found a different app busy on the GPU at the
  wrong moment: a terminal starting, one redrawing itself maximized, a game closing, the shell's
  panel animating itself.
- What held up is a rule more than a number: frames are held only while nothing on screen is
  opening, closing, resizing or animating, the margin is never under half a refresh, and a held
  frame is let go at once if that changes while it waits. That last part was the final bug: a
  frame held just before a window opened was still drawn at its moment, with the new window in it.
- On the way it turned up a bug that had nothing to do with it: a blurred window's backdrop was
  thrown away while the window was on another workspace and rebuilt when it came back, which on
  battery cost that frame its refresh.

Each of these has a test where one was possible; the timing itself, again, only `scripts/check`
on the hardware measures. The frame statistics now log the GPU's time and each frame's latency,
and `pila-check` prints them, so a tester's run shows whether the margin holds on other machines.

## Other projects' rules

Pila takes contributions made with AI help if the pull request says so and the author
understands what they're sending and answers for it. Other projects decide differently, and I
follow their rules. Smithay doesn't accept AI-written text in issues and pull
requests, so what I post there I write myself.

## Check it yourself

```
cargo test
scripts/check --nested    # or scripts/check from a TTY, on real hardware
scripts/coverage
```

If something doesn't hold up, open an issue. It doesn't matter who or what wrote the code.
