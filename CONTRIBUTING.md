# Contributing to Pila

Bug reports, testing on different hardware and distros, and code patches are all welcome. To report
a bug, open an issue using the bug report template, which asks for the details that help most (Pila
version, GPU and distro, how Pila was launched, and logs). AI-assisted contributions are welcome
too; see [Use of AI](README.md#use-of-ai) for the guidelines.

## Conventions

- **English everywhere:** Code, comments, docs, log messages, and commit messages are all in
  English.
- **Standard protocols first:** Prefer Wayland core and official `wayland-protocols`. wlroots
  protocols are accepted only when no standard equivalent exists yet and a desktop tool needs them;
  other vendor protocols (KDE, GNOME, Hyprland, etc.) are not added. See the protocol policy in
  [`CLAUDE.md`](CLAUDE.md).
- **No distro assumptions:** Code and host-side scripts must work on any Linux distribution,
  without assuming systemd (elogind and seatd are supported), and with configurable install paths.
- **Checks before submitting:** Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`. CI runs these plus `shellcheck` on scripts, and also runs distro/Nix/MSRV builds
  and `cargo audit` weekly and whenever dependencies, Nix files, `scripts/install`, or `resources/`
  change.
- **Testing:** Every new feature needs integration tests (`tests/*.rs` on a headless Pila; add new
  tests to `tests/FEATURES.md`), plus unit tests for pure logic. Visual behavior is checked nested
  (`cargo run`), and DRM/KMS behavior on a TTY (`scripts/tty`). `scripts/check` runs a smoke and
  performance workload with real programs (`--nested` in the dev container, or plain on a TTY); add
  a scenario there only for bare-metal-only behavior or features that add rendering load.
- **Regression tests first:** Bug fixes should start with a failing test that reproduces the bug
  (or explain why it can't be automated, such as bare-metal or purely visual bugs). Keep the test
  in the suite.
- **Small commits and synced docs:** Keep commits small, with each one building and passing tests.
  Every setting's default lives as an active line in `config.example.toml` and nowhere else in code
  (verified by a test). Keep `config.example.toml`, `docs/MANUAL.md`, `docs/pila.1`, and `USAGE` in
  `src/main.rs` in sync when adding settings, bindings, or flags (tests check bindings and flags).

## Compatibility

Existing user configs and `pila msg` scripts must continue to work across upgrades:

- **Adding** a config key, value form, action, or IPC field is always fine. Add new keys and
  actions to `tests/fixtures/every-key.toml` (`tests/compat.rs` fails otherwise).
- **Renaming** must keep the old name working via a serde `alias` or a secondary spelling in
  `Action::parse`, and note the new name in `CHANGELOG.md`.
- **Removing** a key or changing its behavior is a breaking change. Avoid it unless strictly necessary,
  document it under its own heading in `CHANGELOG.md` with migration steps, and bump `protocol` for
  IPC changes (`docs/IPC.md`, *Stability guarantees*).
- `tests/fixtures/every-key.toml` and the field lists in `tests/compat.rs` are append-only:
  removing a line there means breaking a real config or script.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the internals: the `Pila` state struct,
backends, render loop, and how protocols and features are wired up.

## Fuzzing

`fuzz/` contains [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for untrusted input
paths. It's a separate nightly-only workspace, invisible to normal builds. Because Pila is split
into a library (`src/lib.rs`) and a binary (`src/main.rs`), fuzz targets call internal parsers
directly.

| Target | Input exercised |
|---|---|
| `ipc_request` | A line of `pila msg` IPC input (`ipc::parse_request`) |
| `config` | A TOML config file with `PILA_*` overrides (`Config::from_toml`, including rules) |
| `edid` | Monitor EDID bytes (`edid::parse`) |
| `gamma_ramp` | A client gamma ramp from a file or open pipe (`gamma_control::read_ramp`) |
| `app_shortcuts` | An `app-shortcuts.json` read back and its triggers, a trigger an app prefers (read, then saved and read again the same), an app id named by its desktop entry (never a path) |
| `sessions` | A `sessions.json` read back (`session_management::parse_sessions`), the sizes and places it restores fitted to a monitor |
| `protocol` | Wayland request sequences (surfaces, buffers, scales, offsets, subsurfaces, viewports, windows, popups, regions, blur regions, window menus after a click) against a running headless Pila |

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run -O config -- -max_total_time=300
# `protocol` runs target/debug/pila (or $PILA_BIN), so build it first:
cargo build && cargo +nightly fuzz run -O protocol -- -max_total_time=300
```

Crash inputs are saved to `fuzz/artifacts/<target>/`. Replay with
`cargo +nightly fuzz run -O <target> <artifact>` and minimize with `cargo +nightly fuzz tmin`. Turn
every crash into a regression test (or an `#[ignore = "bug: ..."]` test if the root cause is
upstream) before fixing it. Note that the `protocol` target only tracks coverage in the fuzzer
process itself rather than the Pila subprocess, so it explores more slowly; give it extra time. The
manual `Fuzz` workflow (`.github/workflows/fuzz.yml`) runs every target for a few minutes and saves
any crash artifacts.

## Development environment

Pila is developed inside a Docker dev container on Linux, running from within Pila (or another
desktop kept as a fallback), with **Claude Code** inside the container.

- **Image:** Arch Linux with stable Rust (`rust-analyzer`, `clippy`, `rustfmt`), `mold`, `gdb`, and
  all build dependencies (winit, DRM/KMS, libinput, PipeWire).
- **Nested rendering:** The host Wayland socket and `/dev/dri` are passed into the container so
  Pila opens as a nested window on your desktop. The login session log is mounted read-only at
  `/run/host-pila`.
- **Included tools:** Test clients (`foot`, `weston-terminal`, `weston-simple-egl`, `wayland-info`,
  `gtk4-widget-factory`, `xterm`) and desktop tools (`kitty`, `fuzzel`, `waybar`, `swaybg`, `mako`,
  `swaylock`, `wl-clipboard`, `grim`, `slurp`, `wtype`, `wlr-randr`, `wlopm`, `wlsunset`, `fcitx5`).
- **Claude Code:** Native install with credentials and history persisted across container rebuilds.

The container connects to whichever desktop session you invoke `scripts/dev` from: run it inside
Pila and nested Pila opens there; run it inside GNOME and it opens on GNOME. Bare-metal changes are
tested on a spare VT with `scripts/tty`, or installed via `scripts/install` and tested in a new
login session (which also ends any running container session). See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for architectural details.

### 1. Host setup

#### Docker

Install Docker from your distro (`docker docker-buildx` on Arch; `docker.io docker-buildx` on Debian
and Ubuntu; `moby-engine` or Docker's repo on Fedora), start the service, and add your user to the
`docker` group:

```bash
sudo systemctl enable --now docker.socket   # systemd; e.g. OpenRC: rc-update add docker
sudo usermod -aG docker "$USER"             # log out and back in afterward
```

Use standard (rootful) Docker. Rootless Docker remaps UIDs and prevents the container from
connecting to your host Wayland socket. Keep in mind that `docker` group membership is effectively
root-equivalent on the host.

#### GPU check

```bash
ls -l /dev/dri
```

Look for `renderD128` with mode `crw-rw-rw-` (the default on most systemd distros). If it's
`crw-rw----` owned by group `render`, add `"--group-add=<GID>"` (check with `getent group render`)
to `runArgs` in `.devcontainer/devcontainer.json`. Intel and AMD GPUs work out of the box through
Mesa. Proprietary NVIDIA requires `nvidia-container-toolkit` and `--gpus=all` in `runArgs`.

#### The Dev Containers CLI

`scripts/dev` uses the Dev Containers CLI (`npm`), which requires Node.js and npm from your distro
(`nodejs npm` on Arch, Debian, Ubuntu, and Fedora):

```bash
npm config set prefix ~/.local     # avoid sudo for global installs; ensure ~/.local/bin is in PATH
npm install -g @devcontainers/cli
```

You can edit files on the host with any editor. Open-source VS Code builds (`Code - OSS`) can't
install Microsoft's *Dev Containers* extension, so the container is driven from the terminal.

### 2. Start the container

Always launch from inside your graphical session so `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are
set:

```bash
scripts/dev up        # initial build takes a few minutes
scripts/dev shell     # open a shell in the container
scripts/dev claude    # start Claude Code in the container
scripts/dev cargo build
```

Startup should print `OK: host Wayland socket mounted` and `OK: GPU render node available`.

### 3. Smoke test

Inside the container:

```bash
wayland-info | head        # talks to the host session through the mounted socket
eglinfo -B                 # should list your GPU, not llvmpipe
weston-simple-egl          # opens a spinning triangle window on your desktop
cargo run                  # opens a nested Pila window
```

### 4. Claude Code

Run `claude` (or `scripts/dev claude`) and sign in. If browser authentication finishes without the
terminal noticing, paste the code shown in the browser into the prompt.

`CLAUDE.md` documents the project context, constraints, and commands. To pre-approve routine
commands, create `.claude/settings.json`:

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

The container runs as a non-root user, but has unrestricted network access. With
`claude --dangerously-skip-permissions`, Claude can edit any file in your checkout and connect to
any network host, so only use it on trusted code and commit frequently.

### Container configuration

| Goal | How |
|---|---|
| Add system packages | Edit the `pacman` line in `.devcontainer/Dockerfile`, then run `scripts/dev rebuild` |
| Enable passwordless sudo | Set the `FULL_SUDO` build arg in `devcontainer.json` (gives container processes root access) |

By default, the `dev` user has no `sudo` privileges.

### Troubleshooting

- **`Cannot connect to Wayland display` / missing socket:** The host compositor restarted (after a
  logout or Pila restart), replacing its Wayland socket. Run any `scripts/dev` command from the
  active session to recreate the link (`scripts/link-wayland`).
- **A tool in the container can't access a host protocol:** When the host is Pila, the mounted
  socket is sandboxed with `wp-security-context` (just like Flatpak), hiding screen capture,
  virtual keyboards, layer shell, and other privileged protocols so container processes can't read
  your screen or inject input. Nested Pila doesn't need those host protocols, and clients running
  inside nested Pila still see all of nested Pila's protocols. If you need direct access to the
  host socket, run with `PILA_DEV_PRIVILEGED=1` (e.g. `PILA_DEV_PRIVILEGED=1 scripts/dev shell`),
  which mounts the unsandboxed session socket instead.
- **Nested Pila opens on the wrong desktop:** The container attaches to whichever session last ran
  `scripts/dev`. Run `scripts/dev` from a terminal in the target session.
- **Nested Pila steals focus or covers your work:** On a Pila host, add a window rule for app ID
  `pila` with `open-on-output` and `open-focused = false` (see *Window and layer rules* in
  `docs/MANUAL.md`).
- **`Permission denied` on the Wayland socket:** Your host UID doesn't match the container user's
  UID. Compare `id -u` on the host and inside the container, and make sure you aren't using rootless
  Docker.
- **`eglinfo` reports `llvmpipe`:** `/dev/dri` isn't mounted into the container, or (on NVIDIA)
  `nvidia-container-toolkit` isn't configured.
- **Super shortcuts go to GNOME:** When running nested under GNOME, GNOME intercepts Super. Nested
  Pila defaults to Alt as Mod for this reason.
