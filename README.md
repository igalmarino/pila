# pila

A stacking Wayland compositor written in Rust on [Smithay](https://github.com/Smithay/smithay).
Windows float and overlap, with server-side title bars, like a classic desktop; panels,
launchers, wallpapers and lock screens come from the usual Wayland tools (waybar, fuzzel,
swaybg, swaylock, ...).

pila implements **standard protocols only**: Wayland core and the official
`wayland-protocols`. The single exception is `zwlr_layer_shell_v1`, which every panel and
launcher needs and has no standard replacement yet.

pila is young but usable day to day: its author runs it as their desktop (one AMD laptop with
an external monitor) and develops it from inside itself.

## Features

- **Windows:** server-side title bars (minimize, maximize, close) and resize borders, or
  client-side decorations for apps that draw their own; Mod+drag to move and resize;
  maximize and fullscreen; snapping to screen halves by keyboard or by dragging to an edge;
  moving, resizing and centering from the keyboard; minimize; switching by recent use
  (Mod+Tab).
- **Monitors:** several at once, hotplugged, each with its own mode, position and
  (fractional) scale.
- **X11 apps** through Xwayland, with the clipboard shared both ways.
- **Desktop tools:** layer shell (waybar, fuzzel, mako, swaybg), session lock (swaylock),
  idle notification and inhibition (swayidle, video players), clipboard managers
  (ext-data-control), screenshots (grim) and screen sharing (xdg-desktop-portal-wlr, for
  browsers and OBS), a visual system bell.
- **Input:** keyboard layout and repeat, touchpad and mouse settings (tap, natural scrolling,
  speed, disable while typing, ...), touchpad gestures for apps, pointer lock and
  confinement for games, any key bindable (media keys included).
- **Configuration** in one TOML file, applied live when saved.

Not there yet: output rotation, touchscreens, input methods (IME), several GPUs.

## Running pila

pila runs on a TTY of a Linux machine (DRM/KMS, libinput, a logind or seatd session), or
nested as a window inside another Wayland or X11 session for development. Build it with
`cargo build --release`; on Arch, besides Rust, it needs `seatd` (for `libseat.so`),
`libinput`, `mesa`, `libxkbcommon` and `xorg-xwayland`.

On a free TTY (Ctrl+Alt+F3), from this folder:

```bash
scripts/tty                        # the release build with ~/.config/pila/config.toml
scripts/tty scripts/tty-test.toml  # another config; --debug for the debug build
```

`scripts/tty` runs pila under its own D-Bus session bus (`dbus-run-session`), so its portals
(screen sharing) don't clash with a GNOME session left running on another VT, which stays a
fallback if pila crashes. The log (portals included) goes to `pila-tty.log`, the previous
run's to `pila-tty.log.old`. Ctrl+Alt+F1..F12 switch TTYs; Mod+Shift+E quits.

Host packages for a full session (Arch):

```bash
sudo pacman -S foot                     # the default terminal (or set `terminal`)
sudo pacman -S xdg-desktop-portal-wlr   # screen sharing; brings grim and slurp
sudo pacman -S waybar swaybg mako swayidle swaylock fuzzel wl-clipboard brightnessctl \
    ttf-nerd-fonts-symbols              # for resources/daily
```

### A day-to-day session

`resources/daily` holds a complete session config: Super as Mod (and Alt+Tab too), a `us`
`altgr-intl` keyboard, touchpad settings like GNOME's, swaybg, mako, swayidle + swaylock,
media, brightness and screenshot keys, a laptop-plus-monitor layout, and a minimal icon-only
waybar with a matching fuzzel theme. `scripts/install-config` copies it to `~/.config/pila/`,
keeping existing files (`--force` replaces them, saving a `.bak`).

## Configuring pila

pila reads `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila/config.toml`, or the file
given with `--config`). [`config.example.toml`](config.example.toml) documents every option
and the default key bindings: the modifier key, terminal and launcher, programs to start with
pila (`autostart`), keyboard layout and repeat, key bindings, touchpad and mouse settings,
and each monitor's mode, scale and position. A missing file means the defaults; an invalid
one is reported in the log and pila starts with the defaults. `PILA_MOD`, `PILA_TERMINAL` and
`PILA_LAUNCHER` override the file; `pila --help` lists the options.

Saving the file applies it right away, everything but `autostart`. If the new version is
invalid, pila keeps running with the previous one and shows the error in a notification
(with a notification daemon such as mako) and in the log.

A few defaults (Mod is Alt unless `mod` says otherwise): Mod+Return opens a terminal,
Mod+Tab switches windows, Mod+Left/Right snap, Mod+Up/Down maximize and restore, Mod+M
maximizes, Mod+F goes fullscreen, Mod+N minimizes, Mod+Shift+Q closes, Mod+Shift+E quits.

## Development environment

pila is developed in a dev container on an **Arch Linux + GNOME** host, with **Claude Code**
running inside the container:

- Arch Linux with Rust stable (rust-analyzer, clippy, rustfmt), mold, gdb, and every library
  pila needs (winit, DRM/KMS, libinput)
- pila runs **nested** as a window on your desktop: the host's Wayland socket and GPU
  (`/dev/dri`) are passed into the container
- Test clients inside: `foot`, `weston-terminal`, `weston-simple-egl`, `wayland-info`,
  `gtk4-widget-factory` (menus and popovers), `xterm` (X11), and desktop tools to test
  against (`fuzzel`, `waybar`, `swaybg`, `mako`, `swaylock`, `wl-clipboard`, `grim`, `slurp`)
- Claude Code (native install) with auth and history persisted across rebuilds
- An egress firewall so Claude Code can only reach crates.io, GitHub, Anthropic and docs.rs

The container follows the session it's entered from: started from GNOME, nested pila opens on
GNOME; entered from a pila session (`scripts/dev` in a terminal there), it opens inside that
pila. See [`CLAUDE.md`](CLAUDE.md) for the architecture.

### 1. Host setup (Arch)

#### Docker

```bash
sudo pacman -S docker docker-buildx
sudo systemctl enable --now docker.socket
sudo usermod -aG docker "$USER"     # log out and back in afterwards
```

Use regular (rootful) Docker. Rootless Docker remaps UIDs, so the container can't connect
to your Wayland socket. Note that membership in the `docker` group is root-equivalent on the
host.

#### GPU check

```bash
ls -l /dev/dri
```

You should see `renderD128` with mode `crw-rw-rw-` (systemd's default on Arch). Intel and AMD
work through Mesa with no extra setup. NVIDIA with the proprietary driver needs the
`nvidia-container-toolkit` package and `--gpus=all` added to `runArgs` in
`.devcontainer/devcontainer.json`.

#### Editor: pick one

**VS Code.** Install `visual-studio-code-bin` from the AUR, then the *Dev Containers*
extension (`ms-vscode-remote.remote-containers`). The `code` package in the official repos is
the open-source build and **can't** install Microsoft's Dev Containers extension.

**Terminal only.** Install the Dev Containers CLI and use the `scripts/dev` helper:

```bash
sudo pacman -S nodejs npm
npm config set prefix ~/.local     # avoid sudo for global installs; ~/.local/bin must be on PATH
npm install -g @devcontainers/cli
```

### 2. Start the container

Always launch from inside your desktop session (GNOME, or pila), so `WAYLAND_DISPLAY` and
`XDG_RUNTIME_DIR` are set on the host.

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
  `scripts/dev` command from the session to link it again (`scripts/link-wayland`); with VS
  Code, reopen the folder in the container.
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

## License

pila is free software: you can redistribute it and/or modify it under the terms of the GNU
General Public License as published by the Free Software Foundation, either version 3 of
the License, or (at your option) any later version. See [LICENSE](LICENSE).
