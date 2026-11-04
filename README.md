# pila

A stacking Wayland compositor written in Rust on [Smithay](https://github.com/Smithay/smithay).
Windows float and overlap, with server-side title bars, like a classic desktop; panels,
launchers, wallpapers and lock screens come from the usual Wayland tools (waybar, fuzzel,
swaybg, swaylock, ...).

pila prefers **standard protocols**: Wayland core and the official `wayland-protocols`.
The wlroots protocols that panels and shells still need and that have no standard
replacement yet (layer shell, screencopy, foreign toplevel management, gamma control,
output management and power management, virtual keyboard) are supported too, each to be
replaced when an `ext-` version lands.

pila is young but usable day to day: its author runs it as their desktop (one AMD laptop with
an external monitor) and develops it from inside itself.

## Features

- **Windows:** server-side title bars (minimize, maximize, close) and resize borders, or
  client-side decorations for apps that draw their own; Mod+drag to move and resize;
  maximize and fullscreen; snapping to screen halves by keyboard or by dragging to an edge;
  moving, resizing and centering from the keyboard; minimize; switching by recent use
  (Mod+Tab).
- **Monitors:** several at once, hotplugged, each with its own mode, position, rotation
  and (fractional) scale.
- **X11 apps** through Xwayland, with the clipboard shared both ways.
- **Desktop tools:** layer shell (waybar, fuzzel, mako, swaybg), session lock (swaylock),
  idle notification and inhibition (swayidle, video players), clipboard managers
  (ext-data-control), screenshots (grim, screencopy) and screen sharing
  (xdg-desktop-portal-wlr, for browsers and OBS), taskbars that act on windows
  (foreign-toplevel-management), night light (gamma control), screen off and monitor
  settings (output power and management), blur behind panels (background-effect), a
  visual system bell. Enough for a full shell such as Noctalia.
- **Input:** keyboard layout and repeat, touchpad and mouse settings (tap, natural scrolling,
  speed, disable while typing, ...), three-finger swipes to switch windows and maximize
  (configurable), other touchpad gestures for apps, pointer lock and confinement for games,
  any key bindable (media keys included).
- **Configuration** in one TOML file, applied live when saved.

Not there yet: touchscreens, input methods (IME), several GPUs.

## Running pila

pila runs on a TTY of any Linux machine (DRM/KMS, libinput, and a session from logind,
elogind or seatd; systemd isn't required), or nested as a window inside another Wayland or
X11 session for development.

### Building

pila needs Rust 1.89 or newer. If your distro's Rust is older, install it with
[rustup](https://rustup.rs). It also needs `pkg-config` and the development files of
`libseat`, `libinput`, `libudev`, `gbm` (Mesa) and `xkbcommon`. At runtime it loads EGL
(Mesa) and starts `Xwayland` for X11 apps. Package names on common distros (they may
differ between releases):

| Distro | Packages |
| - | - |
| Arch | `seatd libinput systemd-libs mesa libxkbcommon xorg-xwayland pkgconf` |
| Debian, Ubuntu | `libseat-dev libinput-dev libudev-dev libgbm-dev libxkbcommon-dev libegl1 xwayland pkg-config` |
| Fedora | `libseat-devel libinput-devel systemd-devel mesa-libgbm-devel libxkbcommon-devel mesa-libEGL xorg-x11-server-Xwayland pkgconf-pkg-config` |
| openSUSE | `libseat-devel libinput-devel libudev-devel libgbm-devel libxkbcommon-devel Mesa-libEGL1 xwayland pkg-config` |
| Void | `libseat-devel libinput-devel eudev-libudev-devel libgbm-devel libxkbcommon-devel mesa xorg-server-xwayland pkg-config` |
| Alpine | `libseat-dev libinput-dev eudev-dev mesa-dev libxkbcommon-dev xwayland pkgconf` |

Then `cargo build --release`.

### Arch Linux (AUR)

`packaging/aur/pila-git` holds a `PKGBUILD` that builds the latest `main` from GitHub and
installs the same files as `scripts/install` with `PREFIX=/usr` (plus the README and
`config.example.toml` in `/usr/share/doc/pila`):

```bash
cd packaging/aur/pila-git && makepkg -si    # or: yay -S pila-git, once it's on the AUR
```

Remove a hand install first (`scripts/uninstall`). To publish or update the AUR package,
regenerate `.SRCINFO` and copy both files into the AUR repository:

```bash
makepkg --printsrcinfo > .SRCINFO
git clone ssh://aur@aur.archlinux.org/pila-git.git /tmp/aur-pila-git
cp PKGBUILD .SRCINFO /tmp/aur-pila-git/
cd /tmp/aur-pila-git && git add PKGBUILD .SRCINFO && git commit -m "Update" && git push
```

A `-git` package only needs updating when the `PKGBUILD` changes (dependencies, files);
`makepkg` picks up new commits by itself.

### As a login session

```bash
scripts/install                # builds the release binary and installs it
PREFIX=/usr scripts/install    # for display managers that only read /usr/share
```

This installs the binary and a session wrapper in `$PREFIX/bin`, a `pila` entry in
`$PREFIX/share/wayland-sessions` for the display manager, and the portal configs for
screen sharing in `$SYSCONFDIR/xdg`. `PREFIX` defaults to `/usr/local` and `SYSCONFDIR`
to `/etc`; `DESTDIR` stages the files for a distro package (see `scripts/install-paths`).
GDM reads `/usr/local`; LightDM and some SDDM setups need `PREFIX=/usr`.

The build uses the host's `cargo`. Without one, it falls back to the dev container
(`--container`), whose binary is linked against Arch's libraries and may not run
elsewhere. `--no-build` installs what's already in `target/release`. Before installing, it
checks that the binary runs on this system (a container build may need a newer glibc than
the host's), and warns if another pila session entry (e.g. a distro package's) is installed
too. Run it on the host, not inside the dev container.

This is the way to install your working tree while developing. The AUR package builds
what's on GitHub, so use it for a published version, and don't keep both installed.

Log out, pick *pila* (on GDM, the gear icon on the password screen), log in. The session
uses the user's D-Bus bus (and `systemd --user` where there is one) like any other
desktop, so portals, PipeWire and the keyring (unlocked by the display manager's PAM setup)
just work. Without a per-user bus (e.g. elogind or seatd without systemd), the wrapper
starts one for the session with `dbus-run-session`.

The log goes to `~/.local/state/pila/pila.log` (the previous run's is in `pila.log.old`).
A copy goes to the journal with systemd (`journalctl -t pila -b`), or otherwise to syslog
through `logger`. After changing pila, run `scripts/install` again and log out and back
in. `scripts/uninstall`, given the same variables, removes what it installed. Mod+Shift+E
ends the session (back to the login screen).

### By hand, on a TTY

On a free TTY (Ctrl+Alt+F3), from this folder:

```bash
scripts/tty                        # the release build with ~/.config/pila/config.toml
scripts/tty scripts/tty-test.toml  # another config; --debug for the debug build
```

This lets you try a build or a config while another session (pila or anything else) keeps
running on another VT. This pila gets a D-Bus bus of its own (`dbus-run-session`), so its
portals don't touch the other session's services. The log (portals included) goes to
`pila-tty.log`, the previous run's to `pila-tty.log.old`. Ctrl+Alt+F1..F12 switch TTYs;
Mod+Shift+E quits.

Host packages for a full session (names are the same on most distros):

- `foot`: the default terminal (or set `terminal`)
- `xdg-desktop-portal-wlr`, `grim`, `slurp`: screen sharing
- `xdg-desktop-portal-gtk`: the file chooser and other portals
- `noctalia`, `ghostty`, `wlopm`, `wl-clipboard`: for `resources/daily`

```bash
sudo pacman -S foot xdg-desktop-portal-wlr xdg-desktop-portal-gtk grim slurp   # Arch
sudo apt install foot xdg-desktop-portal-wlr xdg-desktop-portal-gtk grim slurp # Debian, Ubuntu
sudo dnf install foot xdg-desktop-portal-wlr xdg-desktop-portal-gtk grim slurp # Fedora
```

### A day-to-day session

`resources/daily` holds a complete session config: Super as Mod (and Alt+Tab too), a `us`
`altgr-intl` keyboard, touchpad settings like GNOME's, media, brightness and screenshot
keys, a laptop-plus-monitor layout, and [Noctalia](https://noctalia.dev) as the desktop
shell (bar, launcher, notifications, wallpaper, lock screen and idle, OSDs, clipboard
history, night light, screen off on idle, screenshots, monitor settings), with a Noctalia
settings file adapted to pila (no workspaces widget). `scripts/install-config` copies both
files to `~/.config/`, keeping existing ones (`--force` replaces them, saving a `.bak`).

## Configuring pila

pila reads `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila/config.toml`, or the file
given with `--config`). [`config.example.toml`](config.example.toml) documents every option
and the default key bindings: the modifier key, terminal and launcher, programs to start with
pila (`autostart`), keyboard layout and repeat, key bindings, touchpad and mouse settings,
and each monitor's mode, scale, rotation and position. A missing file means the defaults; an invalid
one is reported in the log and pila starts with the defaults. `PILA_MOD`, `PILA_TERMINAL` and
`PILA_LAUNCHER` override the file; `pila --help` lists the options.

Saving the file applies it right away, everything but `autostart`. If the new version is
invalid, pila keeps running with the previous one and shows the error in a notification
(with a notification daemon such as mako) and in the log.

A few defaults (Mod is Alt unless `mod` says otherwise): Mod+Return opens a terminal,
Mod+Tab switches windows, Mod+Left/Right snap, Mod+Up/Down maximize and restore, Mod+M
maximizes, Mod+F goes fullscreen, Mod+N minimizes, Mod+Shift+S and Mod+Ctrl+S take
screenshots (a region to the clipboard, the screen to `~/Pictures`), Mod+Shift+Q closes,
Mod+Shift+E quits.

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

## License

pila is free software: you can redistribute it and/or modify it under the terms of the GNU
General Public License as published by the Free Software Foundation, either version 3 of
the License, or (at your option) any later version. See [LICENSE](LICENSE).
