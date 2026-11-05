# Contributing to pila

Bug reports, testing on other hardware and distros, and patches are welcome. For a bug,
open an issue with the bug report form; it asks for what helps most (pila's version, your
GPU and distro, how pila was started, and the log).

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
  `cargo test` (CI runs them too, plus builds on Debian, Ubuntu and Fedora and
  `shellcheck` on the scripts). Unit tests cover pure logic; anything needing a client is
  tried nested (`cargo run`), and the DRM/KMS backend by hand on a TTY (`scripts/tty`).
  `scripts/check` runs scripted scenarios (windows, rules, animations, workspaces, a
  launcher, Noctalia if installed) inside a pila and judges them from its log, frame
  statistics included: `--nested` from the dev container, plain from a TTY. A new feature
  adds its scenario and checks there.
- Small commits, each one building and passing the tests. Keep `config.example.toml`, the
  `USAGE` text in `src/main.rs` and the README in step with new settings and flags.

[`CLAUDE.md`](CLAUDE.md) describes the architecture: the state struct, the backends, the
render loop, and how each protocol and feature is wired.

## Development environment

pila is developed in a dev container on any Linux host with Docker, from inside pila itself
(or another desktop, kept as a fallback session), with **Claude Code** running inside the
container. (The author's host is Arch with GNOME as the fallback.)

- An Arch Linux image with Rust stable (rust-analyzer, clippy, rustfmt), mold, gdb, and
  every library pila needs (winit, DRM/KMS, libinput)
- pila runs **nested** as a window on your desktop: the host's Wayland socket and GPU
  (`/dev/dri`) are passed into the container, and the login session's log is mounted at
  `/run/host-pila` so Claude can read it
- Test clients inside: `foot`, `weston-terminal`, `weston-simple-egl`, `wayland-info`,
  `gtk4-widget-factory` (menus and popovers), `xterm` (X11), and desktop tools to test
  against (`fuzzel`, `waybar`, `swaybg`, `mako`, `swaylock`, `wl-clipboard`, `grim`, `slurp`)
- Claude Code (native install) with auth and history persisted across rebuilds
- An egress firewall so Claude Code can only reach crates.io, GitHub, Anthropic and docs.rs

The container follows the session it's entered from: entered from a pila session
(`scripts/dev` in a terminal there), nested pila opens inside that pila; from GNOME (or any
other desktop), on that desktop.
Changes to the compositor itself are tried with `scripts/tty` on a spare VT, or installed
with `scripts/install` and a new login (which ends the container's Claude session too). See
[`CLAUDE.md`](CLAUDE.md) for the architecture.

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

Always launch from inside your desktop session (pila, or any other), so `WAYLAND_DISPLAY` and
`XDG_RUNTIME_DIR` are set on the host.

```bash
scripts/dev up        # first build takes a few minutes
scripts/dev shell     # shell inside the container
scripts/dev claude    # Claude Code inside the container
scripts/dev cargo build
```

The startup log should show `OK: host Wayland socket mounted`, `OK: GPU render node
available` and `Firewall ready`.

### 3. Smoke test

Inside the container:

```bash
wayland-info | head        # talks to the host session through the mounted socket
eglinfo -B                 # should list your GPU, not llvmpipe
weston-simple-egl          # a spinning triangle window appears on your desktop
cargo run                  # nested pila, with a terminal inside
```

### 4. Claude Code

Run `claude` (or `scripts/dev claude`) and sign in. If the browser login completes but the
terminal never notices, paste the code shown in the browser at the prompt.

`CLAUDE.md` gives Claude the project context: nested development, no root, firewall, and the
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

Because the container is non-root with the firewall on, you can run
`claude --dangerously-skip-permissions` for unattended work. Claude can still edit anything in
this folder (it's your real checkout) and reach anything on the allowlist, so only do this with
code you trust and commit often.

### Container configuration

| What | Where |
| - | - |
| Add system packages | `pacman` line in `.devcontainer/Dockerfile`, then rebuild |
| Allow another domain | `.devcontainer/allowed-domains.txt`, then rebuild |
| Start without the firewall | `export DEVCONTAINER_FIREWALL=0` on the host before starting |
| Full passwordless sudo | `FULL_SUDO` build arg in `devcontainer.json` (lets anything in the container disable the firewall) |

The `dev` user can only `sudo` the firewall script, and the allowlist is baked into the image.
That's deliberate: nothing running inside, Claude included, can widen its own network access.

### Troubleshooting

- **`Cannot connect to Wayland display` / socket missing.** The host compositor restarted
  (you logged out and back in, or restarted pila), so its socket is new. Run any
  `scripts/dev` command from the session to link it again (`scripts/link-wayland`).
- **Nested pila opens on the wrong desktop.** The container uses the session `scripts/dev`
  last ran from. Run `scripts/dev` in a terminal of the session you want.
- **`Permission denied` on the socket.** Your host UID isn't the one the container user got.
  Check `id -u` on the host and inside; they must match. Make sure you're not using rootless
  Docker.
- **`eglinfo` shows llvmpipe.** `/dev/dri` isn't passed through, or (NVIDIA) the container
  toolkit isn't set up.
- **A site you need is blocked.** Add it to `allowed-domains.txt` and rebuild.
- **An allowlisted CDN domain (crates.io, docs.rs) fails intermittently.** The firewall allows
  IPs, and CDNs rotate theirs every ~60s. A background loop adds the current IPs every 30s
  (log: `/tmp/firewall-refresh.log`), so retrying after half a minute usually works. To force
  it, run `sudo /usr/local/bin/init-firewall.sh --refresh`, or rerun the script without
  arguments to rebuild the firewall; both are safe while it's active (a failed rebuild keeps
  the previous rules).
- **Super-key shortcuts go to GNOME.** When testing pila nested on GNOME, GNOME grabs Super.
  Nested pila uses Alt as Mod by default for that reason.
