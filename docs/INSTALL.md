# Installing Pila

Pila runs natively on a Linux TTY (using DRM/KMS, libinput, and logind, elogind, or seatd; systemd
is not required) or nested inside a window on an existing Wayland or X11 desktop. Once Pila is
running, see the [user manual](MANUAL.md) for usage and configuration.

- [Try it nested first](#try-it-nested-first)
- [Getting out](#getting-out)
- [Building from source](#building-from-source)
- [Debian, Ubuntu, and Fedora](#debian-ubuntu-and-fedora)
- [Nix and NixOS](#nix-and-nixos)
- [Arch Linux (AUR)](#arch-linux-aur)
- [Installing as a login session](#installing-as-a-login-session)
- [Running manually on a TTY](#running-manually-on-a-tty)
- [Packages for a full session](#packages-for-a-full-session)

## Try it nested first

The safest way to try Pila is to build it (see below) and launch it from your current desktop. It
opens inside a normal window without changing anything else on your system (and launches `waybar`
if installed):

```bash
cargo run --release
```

When running nested, Mod is **Alt** (so your host desktop keeps Super). Press `Mod+Return` to open
a terminal, and `Mod+Shift+E` (or simply closing the window) to quit.

## Getting out

On a TTY or in a login session, Mod is **Super**:

- **Super+Shift+E** quits Pila and returns to your display manager or TTY.
- **Ctrl+Alt+F1..F12** switches to another virtual terminal, where you can log in and run
  `pkill pila` if the screen ever stops responding.
- If the Pila login session fails to start, pick your usual desktop at the login screen and inspect
  the log (see [Reporting bugs](../README.md#reporting-bugs)). Run `scripts/uninstall` (or remove
  the package) to remove the session entry.

## Building from source

Pila requires **Rust 1.89 or newer** (install via [rustup](https://rustup.rs) if your distro ships
an older version). Build dependencies are `pkg-config`, `libclang` (needed to generate PipeWire
bindings at build time), and the development headers for `libseat`, `libinput`, `libudev`, `gbm`
(Mesa), `xkbcommon`, and `libpipewire`. At runtime, Pila loads EGL (Mesa) and optionally launches
`Xwayland` for X11 apps.

Package names on common distros (names may vary slightly between releases):

| Distro | Packages |
| - | - |
| Arch | `seatd libinput systemd-libs mesa libxkbcommon libpipewire clang xorg-xwayland pkgconf` |
| Debian, Ubuntu | `libseat-dev libinput-dev libudev-dev libgbm-dev libxkbcommon-dev libpipewire-0.3-dev libclang-dev libegl1 xwayland pkg-config` |
| Fedora | `libseat-devel libinput-devel systemd-devel mesa-libgbm-devel libxkbcommon-devel pipewire-devel clang-devel mesa-libEGL xorg-x11-server-Xwayland pkgconf-pkg-config` |
| openSUSE | `libseat-devel libinput-devel libudev-devel libgbm-devel libxkbcommon-devel pipewire-devel clang-devel Mesa-libEGL1 xwayland pkg-config` |
| Void | `libseat-devel libinput-devel eudev-libudev-devel libgbm-devel libxkbcommon-devel pipewire-devel clang mesa xorg-server-xwayland pkg-config` |
| Alpine | `libseat-dev libinput-dev eudev-dev mesa-dev libxkbcommon-dev pipewire-dev clang-dev xwayland pkgconf` |

Build with `cargo build --release`, or run `scripts/install` to build and install everything.
`scripts/install` checks for these dependencies before compiling and prints any missing ones (set
`LIBCLANG_PATH` if your `libclang` lives in a non-standard directory).

## Debian, Ubuntu, and Fedora

Signed x86_64 repositories are available for **Ubuntu 24.04+**, **Debian 13+**, and
**Fedora 44+**. Add the repository once and install either `pila-desktop` (Pila plus the
[default session tools](#packages-for-a-full-session)) or `pila` (the compositor only). Updates
then arrive through normal `apt upgrade` or `dnf upgrade`.

**Debian and Ubuntu:**

```bash
sudo curl --create-dirs -fsSLo /etc/apt/keyrings/pila.asc https://igalmarino.github.io/pila/pila.asc
printf 'Types: deb\nURIs: https://igalmarino.github.io/pila/deb\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/pila.asc\n' \
  | sudo tee /etc/apt/sources.list.d/pila.sources
sudo apt update && sudo apt install pila-desktop    # or: pila
```

**Fedora** (`dnf` will prompt to import the signing key on first install; verify its fingerprint
against the one below):

```bash
sudo curl -fsSLo /etc/yum.repos.d/pila.repo https://igalmarino.github.io/pila/rpm/pila.repo
sudo dnf install pila-desktop    # or: pila
```

Repositories and packages are signed with [`packaging/pila.asc`](../packaging/pila.asc),
fingerprint `67A0 F1D1 96AE C2CB B156 0ECA 2413 E5C5 5410 73F3`. The repositories keep the last
three releases. Standalone `.deb` and `.rpm` packages are also attached to every release since
0.2.2 ([GitHub Releases](https://github.com/igalmarino/pila/releases)) for manual installation
(`sudo apt install ./pila_*.deb` or `sudo dnf install ./pila-*.rpm`), though manual installs don't
receive automatic updates.

These packages install the same files as `scripts/install` under `/usr`. Every release package is
tested on a clean system (installed, started headless, and re-verified after publishing with
`pila-desktop`). A tester has confirmed a full login session on Debian with SDDM; Ubuntu and
Fedora still need real-world testing; please [report](../README.md#help-test-pila) how they behave.

## Nix and NixOS

This repository provides a Nix flake tracking `main` (similar to the AUR package). Enable
[flakes](https://wiki.nixos.org/wiki/Flakes) and ensure graphics drivers are enabled on NixOS
(`hardware.graphics.enable = true`).

```bash
nix run github:igalmarino/pila                             # try nested or from a TTY
nix profile install github:igalmarino/pila#pila-desktop    # or: github:igalmarino/pila
```

`pila-desktop` includes Pila and the [default session tools](#packages-for-a-full-session);
`pila` (the default output) provides only the compositor.

To configure Pila as a NixOS login session, add the flake input
(`inputs.pila.url = "github:igalmarino/pila";`) and add the following to your NixOS configuration:

```nix
{ inputs, pkgs, ... }:
let
  pila = inputs.pila.packages.${pkgs.stdenv.hostPlatform.system}.pila-desktop; # or .pila
in
{
  environment.systemPackages = [ pila ];
  services.displayManager.sessionPackages = [ pila ];
  # Required for swaylock (included in pila-desktop) to unlock the screen.
  security.pam.services.swaylock = { };
  # Desktop portals (screen sharing, screenshots, global shortcuts, file chooser).
  xdg.portal = {
    enable = true;
    extraPortals = [
      pila
      pkgs.xdg-desktop-portal-gtk
    ];
    configPackages = [ pila ];
  };
}
```

From a local checkout, `nix develop` drops you into a dev shell with everything needed to run
`cargo build --release`. Don't use `scripts/install` on NixOS, as it installs to FHS paths that
NixOS display managers don't read.

`nix run` has been verified on NixOS, while the full login session config above hasn't been tested
yet. Please [report](../README.md#help-test-pila) your results.

## Arch Linux (AUR)

[`pila-git`](https://aur.archlinux.org/packages/pila-git) builds `main` from GitHub and installs
the same files as `scripts/install` with `PREFIX=/usr` (plus `README.md`, `config.example.toml`,
`MANUAL.md`, and `IPC.md` in `/usr/share/doc/pila`):

```bash
git clone https://aur.archlinux.org/pila-git.git
cd pila-git
less PKGBUILD    # review before building
makepkg -si
```

That installs the compositor alone. [`pila-desktop`](https://aur.archlinux.org/packages/pila-desktop)
pulls in the [default session tools](#packages-for-a-full-session) from the official Arch repos. Repeat
the steps above for `pila-desktop`, or use an AUR helper (`yay -S pila-git pila-desktop`).

To update later, run `git pull && makepkg -si`. You can also build directly from
`packaging/aur/pila-git` in a source checkout. If you previously installed Pila manually with
`scripts/install`, run `scripts/uninstall` first so you don't end up with duplicate installs.

For maintainers publishing AUR updates: regenerate `.SRCINFO`
(`makepkg --printsrcinfo > .SRCINFO`) and commit both files in `packaging/aur/<pkg>/`. The
`aur.yml` workflow validates them and publishes after maintainer approval. Because `pila-git` tracks
`main`, its `PKGBUILD` only needs updating when dependencies or installed files change.

## Installing as a login session

From a source checkout (run on the host, not inside the dev container):

```bash
scripts/install                # build release binary and install to /usr/local
PREFIX=/usr scripts/install    # for display managers that only search /usr/share
```

This installs:
- `pila`, `pila-session`, `pila-report`, and `pila-check` in `$PREFIX/bin` (with `pila-check`
  helpers in `$PREFIX/share/pila/check`)
- the `pila(1)` man page
- the Wayland session entry in `$PREFIX/share/wayland-sessions/pila.desktop`
- Pila's desktop portal backend (`$PREFIX/share/xdg-desktop-portal/portals/pila.portal` and
  `$SYSCONFDIR/xdg/xdg-desktop-portal/pila-portals.conf`)

`PREFIX` defaults to `/usr/local` and `SYSCONFDIR` defaults to `/etc`; set `DESTDIR` to stage files
for packaging (see `scripts/install-paths`). GDM searches `/usr/local`, whereas LightDM and some
SDDM setups require `PREFIX=/usr`.

`scripts/install` uses the host's `cargo` by default, falling back to the dev container
(`--container`) if none is found. Note that a container-built binary links against Arch's libraries
and may fail on distros with an older glibc. Pass `--no-build` to install an existing
`target/release` binary. Before copying files, the script verifies the binary runs on your host and
warns if another Pila session entry (such as a distro package) is already installed.

Use `scripts/install` when developing from your working tree; use a distro package or the AUR for
published versions, and avoid keeping both installed at once.

**Logging in:** Log out, select *Pila* at your display manager (on GDM, click the gear icon on the
password screen), and log in. The session integrates with your user D-Bus session (and
`systemd --user` if available), so portals, PipeWire, and PAM-unlocked keyrings work normally.
Under systemd, Pila runs as `pila.service` bound to `graphical-session.target`, starting your user
services and XDG autostart entries automatically and stopping them on exit. Without a per-user bus
(for example, elogind or seatd without systemd), `pila-session` starts one with `dbus-run-session`.

Logs go to `~/.local/state/pila/pila.log` (or `$XDG_STATE_HOME/pila/pila.log`; the previous run is
saved as `pila.log.old`), and are also mirrored to the systemd journal (`journalctl -t pila -b`) or
syslog via `logger`. After rebuilding Pila, re-run `scripts/install` and log out and back in.
`scripts/uninstall` (with the same `PREFIX`/`SYSCONFDIR` variables) removes all installed files.
Press `Mod+Shift+E` to end the session.

## Running manually on a TTY

Switch to a spare TTY (`Ctrl+Alt+F3`) and run from a source checkout:

```bash
scripts/tty                        # release build with ~/.config/pila/config.toml
scripts/tty my-test.toml           # custom config file (add --debug for debug build)
```

This lets you test a new build or config while your main desktop stays running on another VT.
`scripts/tty` creates an isolated D-Bus session via `dbus-run-session` so portals don't interfere
with your other session. Logs go to `pila-tty.log` (previous run in `pila-tty.log.old`). Switch
TTYs with `Ctrl+Alt+F1..F12` and quit with `Mod+Shift+E`.

## Packages for a full session

Pila is just a compositor. Its default configuration launches standard Wayland tools and quietly
skips any that aren't installed, so you only need to install the ones you want. The `pila-desktop`
metapackage (APT, DNF, AUR, Nix flake) installs all of them together and owns no files of its own,
so removing `pila-desktop` or swapping out individual tools won't disturb Pila.

Default tools (package names are similar across most distros):

- `kitty`: default terminal (change via `terminal`; Pila also auto-detects `foot`, `alacritty`, and
  others if `kitty` isn't installed)
- `waybar`, `fuzzel`, `mako`: default bar, launcher, and notification daemon (plus Font Awesome for
  icons in Pila's default waybar config)
- `swayidle`, `swaylock`: automatic screen locking, idle monitor power-off, and lock-before-sleep
  (unnecessary if you use a desktop shell with its own idle/lock handling)
- `wireplumber` (`wpctl`), `brightnessctl`, `playerctl`: volume, brightness, and media key bindings
- `grim`, `slurp`, `wl-clipboard`: screenshot keybindings
- `swaybg`: wallpaper rendering (without it, Pila displays the solid `[colors] background`)

Recommended session packages (pulled in as Recommends on Debian/Fedora and optional dependencies on
Arch by the `pila` package itself):

- `xdg-desktop-portal` and `pipewire`: screen sharing, app screenshots, and global shortcuts
- `xdg-desktop-portal-gtk`: file chooser and general desktop portal dialogs
- `xdg-utils`: provides `xdg-open` so apps like `kitty` can open links and files
- `Xwayland`: support for X11 applications

To install all of these manually without `pila-desktop`:

```bash
# Arch
sudo pacman -S swaybg kitty waybar fuzzel mako otf-font-awesome xdg-desktop-portal-gtk xdg-utils swayidle swaylock wireplumber brightnessctl playerctl grim slurp wl-clipboard

# Debian, Ubuntu
sudo apt install swaybg kitty waybar fuzzel mako-notifier fonts-font-awesome xdg-desktop-portal-gtk xdg-utils swayidle swaylock wireplumber brightnessctl playerctl grim slurp wl-clipboard

# Fedora
sudo dnf install swaybg kitty waybar fuzzel mako fontawesome-6-free-fonts xdg-desktop-portal-gtk xdg-utils swayidle swaylock wireplumber brightnessctl playerctl grim slurp wl-clipboard
```

For customizing or replacing these tools with your own setup or a full desktop shell, see
[Building your desktop](MANUAL.md#building-your-desktop) in the user manual.
