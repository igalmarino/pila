# Contributing to Pila

Bug reports, testing on other hardware and distros, and patches are welcome. For a bug,
open an issue with the bug report form; it asks for what helps most (Pila's version, your
GPU and distro, how Pila was started, and the log). Changes made with AI help are welcome
too; see [Use of AI](README.md#use-of-ai) for the terms.

## Conventions

- **English** everywhere in the repository: code, comments, docs, log messages, commit
  messages.
- **Standard protocols first**: Wayland core and the official `wayland-protocols`. A
  wlroots protocol is accepted only where no standard one exists yet and a desktop tool
  needs it; other vendor protocols (KDE, GNOME, Hyprland, ...) aren't added. See the
  protocol policy in [`CLAUDE.md`](CLAUDE.md).
- **No distro assumptions**: the code and the host-side scripts must work without systemd
  (elogind or seatd), on any distro, with configurable install paths.
- Before sending a change: `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` (CI runs them too, plus `shellcheck` on the scripts; builds on Debian, Ubuntu
  and Fedora run when dependencies, the Nix files, `scripts/install` or `resources/` change,
  and weekly). Every feature gets integration tests (`tests/*.rs`, on a
  headless Pila; a new test goes into `tests/FEATURES.md`), and unit tests cover pure
  logic. What those can't reach is tried by hand: how things look nested (`cargo run`),
  and the DRM/KMS backend on a TTY (`scripts/tty`). `scripts/check` is a smoke and
  performance run with real programs, judged from Pila's log, frame statistics included:
  `--nested` from the dev container, plain from a TTY. A feature adds a scenario there
  only if it's bare-metal only or puts load on rendering.
- A bug fix starts with a test that reproduces it, seen failing (or says why it can't be
  tested: bare metal, looks); the test stays in the suite.
- Small commits, each one building and passing the tests. Keep `config.example.toml`, the
  user manual (`docs/MANUAL.md`), the man page (`docs/pila.1`) and the `USAGE` text in
  `src/main.rs` in step with new settings, bindings and flags (tests check the bindings and
  flags).

## Compatibility

Users' config files and the programs that talk to `pila msg` must keep working across
versions:

- **Adding** a config key, a value form, an action or an IPC field is free. A new key or
  action also goes into `tests/fixtures/every-key.toml` (`tests/compat.rs` fails until it's
  there).
- **Renaming** keeps the old name working: a serde `alias`, or a second spelling in
  `Action::parse`; the changelog says what the new name is.
- **Removing** or changing what something means is a breaking change: only when nothing
  else works, under its own heading in the changelog with how to update the config, and, for
  `pila msg`, by raising `protocol` (`docs/IPC.md`, "What's guaranteed").
- `tests/fixtures/every-key.toml` and the field lists in `tests/compat.rs` only grow: a line
  taken out there is a config or a script that stops working.

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) describes the architecture: the state struct, the backends, the
render loop, and how each protocol and feature is wired.

## Fuzzing

`fuzz/` has [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for what Pila reads
from outside. It's a workspace of its own, built with nightly Rust; Pila's own builds never see
it. Pila is a library (`src/lib.rs`) plus the `pila` command (`src/main.rs`), so the targets call
its functions directly.

| Target | What it feeds |
|---|---|
| `ipc_request` | a line of `pila msg`'s protocol (`ipc::parse_request`) |
| `config` | a config file and the `PILA_*` overrides (`Config::from_toml`, rules included) |
| `edid` | a monitor's EDID (`edid::parse`) |
| `gamma_ramp` | a client's gamma ramp, from a file or a pipe it may keep open (`gamma_control::read_ramp`) |
| `protocol` | sequences of requests (surfaces, buffers, scales, offsets, subsurfaces, viewports, windows, popups) to a headless Pila it starts once |

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run -O config -- -max_total_time=300
# protocol runs target/debug/pila (or $PILA_BIN): build it first
cargo build && cargo +nightly fuzz run -O protocol -- -max_total_time=300
```

What crashes goes to `fuzz/artifacts/<target>/`; `cargo +nightly fuzz run -O <target>
<artifact>` replays it and `cargo +nightly fuzz tmin` shrinks it. A crash becomes a test that
reproduces it (or a `#[ignore = "bug: ..."]` one, if the fix isn't Pila's), then the fix. The
`protocol` target's coverage is its own, not Pila's (a process of its own), so it explores less
cleverly than the others: give it time. The `Fuzz` workflow (`.github/workflows/fuzz.yml`, run
by hand) runs every target for a few minutes and keeps what crashed.

## Development environment

Pila is developed in a dev container on any Linux host with Docker, from inside Pila itself
(or another desktop, kept as a fallback session), with **Claude Code** running inside the
container. (The author's host is Arch, with Pila as its only desktop.)

- An Arch Linux image with Rust stable (rust-analyzer, clippy, rustfmt), mold, gdb, and
  every library Pila needs (winit, DRM/KMS, libinput)
- Pila runs **nested** as a window on your desktop: the host's Wayland socket and GPU
  (`/dev/dri`) are passed into the container, and the login session's log is mounted at
  `/run/host-pila` so Claude can read it
- Test clients inside: `foot`, `weston-terminal`, `weston-simple-egl`, `wayland-info`,
  `gtk4-widget-factory` (menus and popovers), `xterm` (X11), and desktop tools to test
  against (`kitty`, `fuzzel`, `waybar`, `swaybg`, `mako`, `swaylock`, `wl-clipboard`, `grim`,
  `slurp`, `wtype`, `wlr-randr`, `wlopm`, `wlsunset`, Noctalia and Dux Shell)
- Claude Code (native install) with auth and history persisted across rebuilds

The container follows the session it's entered from: entered from a Pila session
(`scripts/dev` in a terminal there), nested Pila opens inside that Pila; from GNOME (or any
other desktop), on that desktop.
Changes to the compositor itself are tried with `scripts/tty` on a spare VT, or installed
with `scripts/install` and a new login (which ends the container's Claude session too). See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the architecture.

### 1. Host setup

#### Docker

Install Docker from your distro (Arch: `docker docker-buildx`; Debian, Ubuntu:
`docker.io docker-buildx`; Fedora: `moby-engine`, or Docker's own repository), start its
service, and join the `docker` group:

```bash
sudo systemctl enable --now docker.socket   # with systemd; e.g. OpenRC: rc-update add docker
sudo usermod -aG docker "$USER"             # log out and back in afterwards
```

Use regular (rootful) Docker. Rootless Docker remaps UIDs, so the container can't connect
to your Wayland socket. Note that membership in the `docker` group is root-equivalent on the
host.

#### GPU check

```bash
ls -l /dev/dri
```

You should see `renderD128` with mode `crw-rw-rw-` (the default udev rules on most
systemd distros). If it's `crw-rw----` with group `render`, add
`"--group-add=<that group's GID>"` (`getent group render`) to `runArgs` in
`.devcontainer/devcontainer.json`. Intel and AMD work through Mesa with no extra setup. NVIDIA with the proprietary driver needs the
`nvidia-container-toolkit` package and `--gpus=all` added to `runArgs` in
`.devcontainer/devcontainer.json`.

#### The Dev Containers CLI

The container is driven from a terminal with the `scripts/dev` helper, which needs the Dev
Containers CLI, an npm package (Node.js and npm come from your distro: `nodejs npm` on Arch,
Debian, Ubuntu and Fedora):

```bash
npm config set prefix ~/.local     # avoid sudo for global installs; ~/.local/bin must be on PATH
npm install -g @devcontainers/cli
```

Any editor works on the checkout as usual. Distro builds of VS Code (Code - OSS) can't
install Microsoft's *Dev Containers* extension, so the container itself is started and used
from the terminal.

### 2. Start the container

Always launch from inside your desktop session (Pila, or any other), so `WAYLAND_DISPLAY` and
`XDG_RUNTIME_DIR` are set on the host.

```bash
scripts/dev up        # first build takes a few minutes
scripts/dev shell     # shell inside the container
scripts/dev claude    # Claude Code inside the container
scripts/dev cargo build
```

The startup log should show `OK: host Wayland socket mounted` and `OK: GPU render node
available`.

### 3. Smoke test

Inside the container:

```bash
wayland-info | head        # talks to the host session through the mounted socket
eglinfo -B                 # should list your GPU, not llvmpipe
weston-simple-egl          # a spinning triangle window appears on your desktop
cargo run                  # nested Pila, with a terminal inside
```

### 4. Claude Code

Run `claude` (or `scripts/dev claude`) and sign in. If the browser login completes but the
terminal never notices, paste the code shown in the browser at the prompt.

`CLAUDE.md` gives Claude the project context: nested development, no root, and the
commands to use. To pre-approve routine commands, create `.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "Bash(cargo build *)",
      "Bash(cargo check *)",
      "Bash(cargo clippy *)",
      "Bash(cargo fmt *)",
      "Bash(cargo test *)",
      "Bash(git status *)",
      "Bash(git diff *)",
      "Bash(git log *)"
    ]
  }
}
```

The container is non-root, but its network access is unrestricted. With
`claude --dangerously-skip-permissions` Claude can edit anything in this folder (it's your real
checkout) and reach any host, so only do this with code you trust and commit often.

### Container configuration

| What | Where |
| - | - |
| Add system packages | `pacman` line in `.devcontainer/Dockerfile`, then `scripts/dev rebuild` |
| Full passwordless sudo | `FULL_SUDO` build arg in `devcontainer.json` (lets anything in the container act as root) |

The `dev` user has no `sudo` unless `FULL_SUDO` is set.

### Troubleshooting

- **`Cannot connect to Wayland display` / socket missing.** The host compositor restarted
  (you logged out and back in, or restarted Pila), so its socket is new. Run any
  `scripts/dev` command from the session to make it again (`scripts/link-wayland`).
- **A tool in the container can't find a protocol of the host session.** On Pila, the
  container's socket is sandboxed through wp-security-context, as a Flatpak's is: no screen
  capture, virtual keyboard, layer shell or the like, so that nothing in the container can
  read the screen or type into your windows. Nested Pila needs none of that, and tools run
  inside it see all of its protocols. With `PILA_DEV_PRIVILEGED=1` exported in the terminal you
  run `scripts/dev` from (it makes the socket again on every command), e.g.
  `PILA_DEV_PRIVILEGED=1 scripts/dev shell`, the container shares the session's own socket
  instead, and everything in it gets those powers.
- **Nested Pila opens on the wrong desktop.** The container uses the session `scripts/dev`
  last ran from. Run `scripts/dev` in a terminal of the session you want.
- **`Permission denied` on the socket.** Your host UID isn't the one the container user got.
  Check `id -u` on the host and inside; they must match. Make sure you're not using rootless
  Docker.
- **`eglinfo` shows llvmpipe.** `/dev/dri` isn't passed through, or (NVIDIA) the container
  toolkit isn't set up.
- **Super-key shortcuts go to GNOME.** When testing Pila nested on GNOME, GNOME grabs Super.
  Nested Pila uses Alt as Mod by default for that reason.
