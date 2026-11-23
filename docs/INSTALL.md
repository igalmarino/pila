# Installing Pila

Pila runs on a TTY of any Linux machine (DRM/KMS, libinput, and a session from logind,
elogind or seatd; systemd isn't required), or nested as a window inside another Wayland or
X11 session. Once it runs, how to use and set it up is in the [user manual](MANUAL.md).

- [Try it nested first](#try-it-nested-first)
- [Getting out](#getting-out)
- [Building](#building)
- [Debian, Ubuntu and Fedora](#debian-ubuntu-and-fedora)
- [Nix and NixOS](#nix-and-nixos)
- [Arch Linux (AUR)](#arch-linux-aur)
- [As a login session](#as-a-login-session)
- [By hand, on a TTY](#by-hand-on-a-tty)
- [Packages for a full session](#packages-for-a-full-session)

## Try it nested first

The safest first run: build it (below) and start it from your current desktop. It opens
as a window, and nothing else on your system changes (the window starts with the default bar,
waybar, where it's installed):

```bash
cargo run --release
```

Nested, Mod is **Alt** (your desktop keeps Super). Mod+Return opens a terminal, Mod+Shift+E
quits. Closing the window works too.

## Getting out

On a TTY or as a login session, Mod is **Super**:

- **Super+Shift+E** quits Pila (back to the login screen or the TTY).
- **Ctrl+Alt+F1..F12** switch to another virtual terminal, where you can log in and
  `pkill pila` if the screen stops responding.
- If the Pila session doesn't start, pick your usual session at the login screen and
  look at the log (see [Reporting bugs](../README.md#reporting-bugs)); `scripts/uninstall` (or
  removing the package) takes the session entry away.

## Building

Pila needs Rust 1.89 or newer. If your distro's Rust is older, install it with
[rustup](https://rustup.rs). It also needs `pkg-config` and the development files of
`libseat`, `libinput`, `libudev`, `gbm` (Mesa), `xkbcommon` and `libpipewire`, and libclang
(its PipeWire bindings are generated at build time). At runtime it loads EGL
(Mesa) and, for X11 apps, starts `Xwayland` if it's installed (optional). Package names on common distros (they may
differ between releases):

| Distro | Packages |
| - | - |
| Arch | `seatd libinput systemd-libs mesa libxkbcommon libpipewire clang xorg-xwayland pkgconf` |
| Debian, Ubuntu | `libseat-dev libinput-dev libudev-dev libgbm-dev libxkbcommon-dev libpipewire-0.3-dev libclang-dev libegl1 xwayland pkg-config` |
| Fedora | `libseat-devel libinput-devel systemd-devel mesa-libgbm-devel libxkbcommon-devel pipewire-devel clang-devel mesa-libEGL xorg-x11-server-Xwayland pkgconf-pkg-config` |
| openSUSE | `libseat-devel libinput-devel libudev-devel libgbm-devel libxkbcommon-devel pipewire-devel clang-devel Mesa-libEGL1 xwayland pkg-config` |
| Void | `libseat-devel libinput-devel eudev-libudev-devel libgbm-devel libxkbcommon-devel pipewire-devel clang mesa xorg-server-xwayland pkg-config` |
| Alpine | `libseat-dev libinput-dev eudev-dev mesa-dev libxkbcommon-dev pipewire-dev clang-dev xwayland pkgconf` |

Then `cargo build --release`; `scripts/install` builds and installs it, and checks first that
these are there (it names the ones missing; a libclang elsewhere: `LIBCLANG_PATH`).

## Debian, Ubuntu and Fedora

Pila has an APT repository (Ubuntu 24.04 or newer, Debian 13 or newer) and a DNF repository
(Fedora 44 or newer), for x86_64. Add one once and install `pila-desktop` (Pila with the
[tools of a full session](#packages-for-a-full-session)) or `pila` (the compositor alone);
updates then come with the system's (`apt upgrade`, `dnf upgrade`).

Debian, Ubuntu:

```bash
sudo curl --create-dirs -fsSLo /etc/apt/keyrings/pila.asc https://igalmarino.github.io/pila/pila.asc
printf 'Types: deb\nURIs: https://igalmarino.github.io/pila/deb\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/pila.asc\n' \
  | sudo tee /etc/apt/sources.list.d/pila.sources
sudo apt update && sudo apt install pila-desktop    # or: pila
```

Fedora (`dnf` asks to import the key the first time: compare its fingerprint with the one below):

```bash
sudo curl -fsSLo /etc/yum.repos.d/pila.repo https://igalmarino.github.io/pila/rpm/pila.repo
sudo dnf install pila-desktop    # or: pila
```

The repositories and their packages are signed with the key
[`packaging/pila.asc`](../packaging/pila.asc), fingerprint
`67A0 F1D1 96AE C2CB B156 0ECA 2413 E5C5 5410 73F3`. They hold the packages of the last three
releases; every release since 0.2.2 also has its `.deb` and `.rpm` attached
([releases](https://github.com/igalmarino/pila/releases)), to install by hand
(`sudo apt install ./pila_*.deb`, `sudo dnf install ./pila-*.rpm`) with no updates.

They install the same files as `scripts/install` in `/usr`. Each package is installed on a clean
system and started headless before it's attached to a release, and installed again from the
repositories once they're published, with `pila-desktop` and its tools. A tester reports a real session working on Debian, with
SDDM; on Ubuntu and Fedora it hasn't been tried yet: please [report](../README.md#help-test-pila) how it goes.

## Nix and NixOS

The repository is a flake, which builds the latest `main`, like the AUR package. It needs
[flakes enabled](https://wiki.nixos.org/wiki/Flakes) and, on NixOS, graphics drivers on the
system (`hardware.graphics.enable = true`).

```bash
nix run github:igalmarino/pila           # try it: nested from your desktop, or from a TTY
nix profile install github:igalmarino/pila#pila-desktop    # or: github:igalmarino/pila
```

`pila-desktop` is Pila with the [tools of a full session](#packages-for-a-full-session),
`pila` (the default) the compositor alone.

As a login session on NixOS, add the flake to your system's inputs
(`inputs.pila.url = "github:igalmarino/pila";`) and to its configuration:

```nix
{ inputs, pkgs, ... }:
let
  pila = inputs.pila.packages.${pkgs.stdenv.hostPlatform.system}.pila-desktop; # or .pila
in
{
  environment.systemPackages = [ pila ];
  services.displayManager.sessionPackages = [ pila ];
  # swaylock (in pila-desktop) needs it to unlock.
  security.pam.services.swaylock = { };
  # Screen sharing.
  xdg.portal = {
    enable = true;
    wlr.enable = true;
    configPackages = [ pila ];
  };
}
```

From a checkout, `nix develop` gives a shell with everything to build it (`cargo build
--release`). `scripts/install` isn't for NixOS: it writes to paths NixOS doesn't read its session
entries from.

`nix run` has been confirmed on a NixOS system. The login session above hasn't been tried yet:
please [report](../README.md#help-test-pila) how it goes.

## Arch Linux (AUR)

[`pila-git`](https://aur.archlinux.org/packages/pila-git) builds the latest `main` from
GitHub and installs the same files as `scripts/install` with `PREFIX=/usr` (plus the README,
`config.example.toml`, the user manual and `docs/IPC.md` in `/usr/share/doc/pila`):

```bash
git clone https://aur.archlinux.org/pila-git.git
cd pila-git
less PKGBUILD    # read what you're about to build and install
makepkg -si
```

That's the compositor alone. [`pila-desktop`](https://aur.archlinux.org/packages/pila-desktop)
adds the [tools of a full session](#packages-for-a-full-session), all from Arch's own
repositories: after `pila-git`, the same steps with `pila-desktop` (or an AUR helper:
`yay -S pila-git pila-desktop`).

To update it, `git pull` there and run `makepkg -si` again. `makepkg -si` in
`packaging/aur/pila-git` builds it too. Remove a hand install first
(`scripts/uninstall`). To publish a new version of an AUR package, regenerate its `.SRCINFO`
(`makepkg --printsrcinfo > .SRCINFO`) and commit both files: a workflow (`aur.yml`) checks them
and, once the maintainer approves it, publishes them.

A `-git` package only needs updating when the `PKGBUILD` changes (dependencies, files);
`makepkg` picks up new commits by itself. Their `PKGBUILD`s are kept in `packaging/aur`, one
directory for each.

## As a login session

From Pila's source:

```bash
scripts/install                # builds the release binary and installs it
PREFIX=/usr scripts/install    # for display managers that only read /usr/share
```

This installs the binary, a session wrapper, `pila-report` and `pila-check` in `$PREFIX/bin`
(`pila-check`'s helpers in `$PREFIX/share/pila/check`), the man page,
a `pila` entry in `$PREFIX/share/wayland-sessions` for the display manager, and the portal
configs for screen sharing in `$SYSCONFDIR/xdg`. `PREFIX` defaults to `/usr/local` and `SYSCONFDIR`
to `/etc`; `DESTDIR` stages the files for a distro package (see `scripts/install-paths`).
GDM reads `/usr/local`; LightDM and some SDDM setups need `PREFIX=/usr`.

The build uses the host's `cargo`. Without one, it falls back to the dev container
(`--container`), whose binary is linked against Arch's libraries and may not run
elsewhere. `--no-build` installs what's already in `target/release`. Before installing, it
checks that the binary runs on this system (a container build may need a newer glibc than
the host's), and warns if another Pila session entry (e.g. a distro package's) is installed
too. Run it on the host, not inside the dev container.

This is the way to install your working tree while developing. The AUR package builds
what's on GitHub, so use it for a published version, and don't keep both installed.

Log out, pick *Pila* (on GDM, the gear icon on the password screen), log in. The session
uses the user's D-Bus bus (and `systemd --user` where there is one) like any other
desktop, so portals, PipeWire and the keyring (unlocked by the display manager's PAM setup)
just work. With systemd, Pila runs as the user service `pila.service`, bound to
`graphical-session.target`, so the user services of the graphical session and the XDG
autostart entries start with it and stop when it quits. Without a per-user bus (e.g. elogind
or seatd without systemd), the wrapper starts one for the session with `dbus-run-session`.

The log goes to `~/.local/state/pila/pila.log` (`$XDG_STATE_HOME/pila` if that's set; the
previous run's is in `pila.log.old`).
A copy goes to the journal with systemd (`journalctl -t pila -b`), or otherwise to syslog
through `logger`. After changing Pila, run `scripts/install` again and log out and back
in. `scripts/uninstall`, given the same variables, removes what it installed. Mod+Shift+E
ends the session (back to the login screen).

## By hand, on a TTY

On a free TTY (Ctrl+Alt+F3), from Pila's source:

```bash
scripts/tty                        # the release build with ~/.config/pila/config.toml
scripts/tty my-test.toml           # another config; --debug for the debug build
```

This lets you try a build or a config while another session (Pila or anything else) keeps
running on another VT. This Pila gets a D-Bus bus of its own (`dbus-run-session`), so its
portals don't touch the other session's services. The log (portals included) goes to
`pila-tty.log`, the previous run's to `pila-tty.log.old`. Ctrl+Alt+F1..F12 switch TTYs;
Mod+Shift+E quits.

## Packages for a full session

Pila is a compositor alone: its default configuration runs standard tools for the rest of a
desktop, and skips those that aren't installed, so you install only the ones you want. The
package `pila-desktop` (APT, DNF, the AUR and the flake) brings them all at once; it has no files
of its own, so removing it, or any of them, leaves Pila as it was. The programs (names are the
same on most distros):

- `kitty`: the default terminal (or set `terminal`; foot, alacritty and others are found
  too)
- `waybar`, `fuzzel` and `mako`: the default bar, launcher and notification daemon (or set
  `autostart` and `launcher` to others); Font Awesome for the symbols in the waybar
  configuration Pila writes
- `swayidle` and `swaylock`: locking the screen and turning the monitors off after a while
  without input, and locking before the computer sleeps (the default `autostart`); a shell
  with its own lock screen and idle settings doesn't need them
- `wireplumber` (PipeWire's session manager, which most distros install with PipeWire):
  `wpctl`, which the volume and mute keys use; `brightnessctl` for the brightness keys and
  `playerctl` for the media keys; without them the keys do nothing
- `grim`, `slurp` and `wl-clipboard`: the screenshot keys
- `swaybg`: the wallpaper, Pila's in your `[colors]` or a picture of yours (without it,
  `[colors] background` shows)

Apps also want these in any session, so `pila` itself recommends them (Debian and Fedora
install them with it unless recommends are off; on Arch they're optional dependencies):

- `xdg-desktop-portal` and `pipewire`: screen sharing, which Pila serves itself (it asks which
  screen to share and streams it over PipeWire)
- `xdg-desktop-portal-wlr`: screenshots through the portal
- `xdg-desktop-portal-gtk`: the file chooser and other portals
- `xdg-utils`: `xdg-open`, which programs such as kitty use to open links and files; without it
  a click on a link does nothing, and says nothing
- Xwayland: X11 apps

By hand, without `pila-desktop` (the portals and `xdg-utils` included):

```bash
sudo pacman -S kitty waybar fuzzel mako otf-font-awesome xdg-desktop-portal-wlr xdg-desktop-portal-gtk xdg-utils swayidle swaylock wireplumber brightnessctl playerctl grim slurp wl-clipboard   # Arch
sudo apt install kitty waybar fuzzel mako-notifier fonts-font-awesome xdg-desktop-portal-wlr xdg-desktop-portal-gtk xdg-utils swayidle swaylock wireplumber brightnessctl playerctl grim slurp wl-clipboard # Debian, Ubuntu
sudo dnf install kitty waybar fuzzel mako fontawesome-6-free-fonts xdg-desktop-portal-wlr xdg-desktop-portal-gtk xdg-utils swayidle swaylock wireplumber brightnessctl playerctl grim slurp wl-clipboard  # Fedora
```

The rest of a desktop (notifications, a lock screen, another bar or launcher)
is in the manual's [Building your desktop](MANUAL.md#building-your-desktop).
