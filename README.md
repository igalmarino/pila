# Wayland compositor: dev environment

A dev container for building a Rust/Smithay Wayland compositor on an **Arch Linux + GNOME**
host, with **Claude Code** running inside the container.

What you get:

- Arch Linux container with Rust stable (rust-analyzer, clippy, rustfmt), mold, gdb, and
  every library Smithay needs (winit now, DRM/libinput later)
- Your compositor runs **nested** as a window on your GNOME desktop: the host's Wayland
  socket and GPU (`/dev/dri`) are passed into the container
- Test clients inside: `foot`, `weston-terminal`, `weston-simple-egl`, `wayland-info`,
  `gtk4-widget-factory` (menus and popovers), and desktop tools to test against (`fuzzel`,
  `waybar`, `swaybg`, `mako`, `swaylock`, `wl-clipboard`)
- Claude Code (native install) with auth and history persisted across rebuilds
- An egress firewall so Claude Code can only reach crates.io, GitHub, Anthropic and docs.rs

## 1. Host setup (Arch)

### Docker

```bash
sudo pacman -S docker docker-buildx
sudo systemctl enable --now docker.socket
sudo usermod -aG docker "$USER"     # log out and back in afterwards
```

Use regular (rootful) Docker. Rootless Docker remaps UIDs, so the container can't connect
to your Wayland socket. Note that membership in the `docker` group is root-equivalent on the
host.

### GPU check

```bash
ls -l /dev/dri
```

You should see `renderD128` with mode `crw-rw-rw-` (systemd's default on Arch). Intel and AMD
work through Mesa with no extra setup. NVIDIA with the proprietary driver needs the
`nvidia-container-toolkit` package and `--gpus=all` added to `runArgs` in
`.devcontainer/devcontainer.json`.

### Editor: pick one

**VS Code.** Install `visual-studio-code-bin` from the AUR, then the *Dev Containers*
extension (`ms-vscode-remote.remote-containers`). The `code` package in the official repos is
the open-source build and **can't** install Microsoft's Dev Containers extension.

**Terminal only.** Install the Dev Containers CLI and use the `scripts/dev` helper:

```bash
sudo pacman -S nodejs npm
npm config set prefix ~/.local     # avoid sudo for global installs; ~/.local/bin must be on PATH
npm install -g @devcontainers/cli
```

## 2. Start the container

Always launch from inside your GNOME session, so `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are
set on the host.

**VS Code:** open this folder, then *Dev Containers: Reopen in Container*.

**Terminal:**

```bash
scripts/dev up        # first build takes a few minutes
scripts/dev shell     # shell inside the container
scripts/dev claude    # Claude Code inside the container
scripts/dev cargo build
```

The startup log should show `OK: host Wayland socket mounted`, `OK: GPU render node
available` and `Firewall ready`.

## 3. Smoke test

Inside the container:

```bash
wayland-info | head        # talks to GNOME through the mounted socket
eglinfo -B                 # should list your GPU, not llvmpipe
weston-simple-egl          # a spinning triangle window appears on your GNOME desktop
```

If all three work, a Smithay winit compositor will run nested with GPU rendering.

## 4. Claude Code

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

## Configuration

| What | Where |
| - | - |
| Add system packages | `pacman` line in `.devcontainer/Dockerfile`, then rebuild |
| Allow another domain | `.devcontainer/allowed-domains.txt`, then rebuild |
| Start without the firewall | `export DEVCONTAINER_FIREWALL=0` on the host before starting |
| Full passwordless sudo | `FULL_SUDO` build arg in `devcontainer.json` (lets anything in the container disable the firewall) |

The `dev` user can only `sudo` the firewall script, and the allowlist is baked into the image.
That's deliberate: nothing running inside, Claude included, can widen its own network access.

## Troubleshooting

- **`Cannot connect to Wayland display` / socket missing.** The container was started outside
  your GNOME session, or you logged out and back in (GNOME recreates the socket). Restart the
  container.
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
- **Super-key shortcuts go to GNOME.** When testing your nested compositor, GNOME grabs Super.
  Use Alt-based bindings while nested.

## Configuring pila

pila reads `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila/config.toml`, or the
file given with `--config`). [`config.example.toml`](config.example.toml) documents every
option: the modifier key, terminal and launcher, programs to start with pila (`autostart`),
keyboard layout and repeat, and key bindings (including media keys). A missing file means
the defaults; an invalid one is reported in the log and pila starts with the defaults.
`PILA_MOD`, `PILA_TERMINAL` and `PILA_LAUNCHER` override the file.

## Running pila on a TTY (real hardware)

The nested winit backend covers most development. The DRM/KMS + libinput backend
(`--backend udev`, the default when no `WAYLAND_DISPLAY`/`DISPLAY` is set) runs on the
**host** from a TTY, since it needs a seat and direct device access. Cargo builds in the
container land in `target/` in this folder, so the host can run the binary directly.

Host packages the binary needs beyond a GNOME install:

```bash
sudo pacman -S seatd    # libseat.so; the seatd daemon isn't needed, libseat uses logind
sudo pacman -S foot     # pila's default terminal (or set PILA_TERMINAL, e.g. kgx)
sudo pacman -S xdg-desktop-portal-wlr   # screen sharing (brings grim and slurp)
```

Build the release binary in the container (`scripts/dev cargo build --release`), then on a
free TTY (Ctrl+Alt+F3) log in and from this folder run:

```bash
scripts/tty                        # ~/.config/pila/config.toml
scripts/tty scripts/tty-test.toml  # or another config; --debug for the debug build
```

It runs pila under its own D-Bus session bus, so its portals (screen sharing) don't clash
with a GNOME session left running on another VT, which stays a fallback if pila crashes.
The log (stderr, portals included) lands in `pila-tty.log` in the repo, the previous run's in
`pila-tty.log.old`, where Claude Code can read them (`*.log*` is git-ignored).
Ctrl+Alt+F1..F12 switch TTYs, Mod+Shift+E quits.

### Day to day

`resources/daily` holds a session config: Super as Mod, `us` `altgr-intl`, a minimal waybar, swaybg,
mako, swayidle + swaylock, media and brightness keys, screenshots, and the laptop and monitor
layout. Install it with `scripts/install-config` (it never overwrites) and the tools with:

```bash
sudo pacman -S waybar swaybg mako swayidle swaylock fuzzel wl-clipboard brightnessctl \
    ttf-nerd-fonts-symbols
```

The dev container works from inside pila too: `scripts/dev` links the Wayland socket of the
session it runs in (`scripts/link-wayland`), so nested pila opens inside the running pila.
