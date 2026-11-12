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

## Who I am

I started programming as a kid on a Commodore 64, in BASIC and 6510 assembly. I used C and
Pascal at university and have worked in software for more than 30 years since, in PHP, classic
ASP (VBScript), Perl, Python, Java and JavaScript/TypeScript, among others. For the last several
years I've worked mostly as a software architect, and lately as CTO of a company.

I had never written Rust or a Wayland compositor before Pila. What I do know is how to design a
system and how to decide what has to be tested before I trust it, so the project depends on
tests and measurements more than on me reading every line.

I started Pila because I wanted a classic desktop with overlapping windows for Noctalia and Dux
Shell. Both usually run on tiling compositors, and the stacking compositors I tried didn't work
with them the way I wanted ([What sets Pila apart](WHY-PILA.md) has the comparison). I also
wanted a real project to get better at Rust.

## Who decides

I do. Scope, technology, architecture, which protocols Pila supports, the security model, the
performance budget and what counts as tested are my decisions, and most of them are written
down in [CLAUDE.md](../CLAUDE.md). I review what goes in, I use Pila as my only desktop, and I
answer for it.

Reviewing, for me, means this. I read the code that can't be checked from outside: input
handling, the session lock, what sandboxed clients can see, the DRM/KMS backend. For everything
else I read the tests first, since code that passes a wrong test proves nothing. Visual changes
and anything that depends on real hardware aren't committed until I've seen them work. When I
don't like what Claude Code wrote, I discard it and it gets implemented again.

## What the AI does

Claude Code writes most of the code, the tests and the documentation from my instructions. It
also translates what I write into English, and this page went through it too.

It runs in a dev container without root, where Pila runs nested as a window on my desktop.
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
control socket. There are about 650 integration tests in 34 files and about 150 unit tests, and
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
- While the session is locked, a code path that would give input to anything but the lock
  screen panics in debug builds.
- A test fails if the docs name a file or a function that no longer exists.
- `tests/compat.rs` keeps a config file with every key Pila has ever accepted, so a change that
  would break someone's config or `pila msg` script fails.

### Performance

Pila shouldn't miss a refresh while there's something to show, at whatever rate the monitor
runs, and its CPU time per frame has to average under 4 ms (a quarter of a 60 Hz period).
`frame_stats.rs` logs frames, average and worst CPU time and missed refreshes per output. On
bare metal the missed refreshes are exact, taken from the DRM vblank sequence. `scripts/check`
runs a release build through real programs (terminals, xterm, a launcher, screenshots, a screen
share, blur, a fullscreen app, Noctalia) and fails on a missed refresh or a blown budget.
Changes to rendering, input or the event loop go through it before they're committed.

Last bare-metal run, 2026-11-12, on an AMD Renoir laptop on mains power with an external
monitor: 23 five-second windows over both outputs, 0.24 to 1.0 ms of CPU per frame on average,
no missed or late refresh in 1,880 frames. The slowest frame took 11.7 ms, when a fullscreen
terminal left direct scanout. It still made its vblank, but that delay is a known problem
(see "Late frames on my laptop" below).

### Real hardware

The DRM/KMS backend can't run in the container, so I run `scripts/check` on a spare TTY of my
laptop. Pila has been my only desktop since 2026-11-03. I log into it from the greeter and
develop it from inside. Testers have reported it working on Intel GPUs, Debian with SDDM, NixOS
and LightDM. The README lists what nobody has tried yet: NVIDIA, systems without systemd,
monitors on a second GPU.

### Shells

`scripts/shells` starts Noctalia and Dux Shell on a sealed-off headless Pila and drives each
one through its own IPC: panels, launcher, workspaces, taskbar, notifications, clipboard,
screenshots, gamma, output management and power, lock screen.

### CI

Every push runs `cargo fmt`, `clippy -D warnings`, a `--locked` build, a staged install and
`shellcheck`. Once a week, and whenever dependencies change, it builds on Debian, Ubuntu, Fedora
and Nix and with the minimum supported Rust, and runs `cargo audit`. There are fuzz targets for
the config parser, EDID, gamma ramps, IPC requests and the Wayland protocol, and an experimental
job that runs the DRM/KMS backend on vkms, the kernel's virtual KMS driver.

### Documentation

[docs/ARCHITECTURE.md](ARCHITECTURE.md) has a section per subsystem with its flows and
invariants. A change to a flow updates its section in the same commit. I need it as much as the
AI does: nobody keeps 36,000 lines in their head.

## About the speed

The first commit is from 2026-10-31. Less than two weeks later Pila was a daily-driver alpha
with about 36,000 lines of Rust and 20,000 of tests. Nobody writes that by hand in two weeks,
and I'm not going to pretend I did.

That speed is only useful if checking keeps up with it. Whatever goes in still has to pass the
tests, stay within the frame budget and, when it touches rendering, run on my hardware first.

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

A tester on NixOS (issue #1) got a panic when `libEGL` couldn't be loaded. It worked on my
machine and in CI, so it took a user on another distribution to find it. Pila now checks at
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
`scripts/check` got scenarios for them, and the fix was checked nested and on my hardware before
it went in.

### Late frames on my laptop

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
  with a single monitor. This one is measured and documented, and still open.
- The single-monitor run was on battery, because the monitor's cable also charges the laptop.
  Everything cost about twice as much, and the most expensive frame (opening a window) no longer
  fit in its margin. That's not a bug in Pila, but it does mean the 4 ms budget is a mains-power
  number.

The run after the fix was clean. One clean run proves little, and two of those three causes are
still there.

Where a check was possible, each of these fixes added one, so the same mistake fails the suite
next time. The late frames are the exception: that's timing on real hardware, and only
`scripts/check` measures it.

## Other projects' rules

Pila takes contributions made with AI help if the pull request says so and the author
understands what they're sending and answers for it. Other projects decide differently, and I
follow their rules. Smithay and Dux Shell don't accept AI-written text in issues and pull
requests, so what I post there I write myself.

## Check it yourself

```
cargo test
scripts/check --nested    # or scripts/check from a TTY, on real hardware
scripts/shells
scripts/coverage
```

If something doesn't hold up, open an issue. It doesn't matter who or what wrote the code.
