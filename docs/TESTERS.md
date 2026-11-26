# Testers

Pila is developed on a single machine, so everything known about how it runs elsewhere comes from
the people below. They ran it on their own hardware and setups, sent logs and `pila-check` runs,
and asked for what was missing. Thank you.

| Tester | Hardware and setup | What their reports led to |
|---|---|---|
| [1Hung1](https://github.com/1Hung1) | Intel GPU; NixOS, and Debian with SDDM | The first run of Pila outside the development machine; the Nix flake and `libEGL` linked into the binary; `PILA_BIN` to run `pila-check` against any build; the first login session on Debian ([issue #1](https://github.com/igalmarino/pila/issues/1)) |
| [qinohe](https://github.com/qinohe) | Intel NUC 10 (Core i7-10710U), Arch Linux; an Arch VM with LightDM | `pila-git` built from the PKGBUILD; Pila started from a login script, with no display manager; Alt+Tab among the default bindings; focus for dmenu-style launchers ([#5](https://github.com/igalmarino/pila/issues/5)); snapping to the top and bottom halves ([#4](https://github.com/igalmarino/pila/discussions/4)) |
| Tavo (Telegram) | NVIDIA GTX 1060, NixOS | The first NVIDIA report; `pila-check` fixes for Nix's wrapped binary and Pythons without `os.memfd_create` (0.5.3) |
| cristoalv (Telegram) | AMD Radeon RX 6800 XT | The manual's list of desktop tools that work on Pila, and its waybar setup (`ext/workspaces`) |
| [ffs-akl](https://github.com/ffs-akl) | Arch Linux (AUR) | Input methods ([#2](https://github.com/igalmarino/pila/discussions/2)); optional Xwayland ([#3](https://github.com/igalmarino/pila/discussions/3)); cursor theme and size ([#7](https://github.com/igalmarino/pila/discussions/7)); blur on popups ([#8](https://github.com/igalmarino/pila/discussions/8)); rounded window corners ([#11](https://github.com/igalmarino/pila/discussions/11)); the built-in ScreenCast portal, tested with Kooha ([#12](https://github.com/igalmarino/pila/discussions/12), [#14](https://github.com/igalmarino/pila/discussions/14), [#15](https://github.com/igalmarino/pila/discussions/15), [#17](https://github.com/igalmarino/pila/discussions/17)); building with LTO ([#13](https://github.com/igalmarino/pila/discussions/13)); hot-corner settings ([#16](https://github.com/igalmarino/pila/discussions/16)) |

## Joining

Hardware Pila hasn't met yet is the most useful: newer NVIDIA cards, dual-GPU laptops, systems
without systemd, high refresh rates and mixed monitor scaling. Run `pila-check` from a spare TTY
and send a [test report](https://github.com/igalmarino/pila/issues/new?template=test_report.yml),
even when everything passes, or comment on [issue #1](https://github.com/igalmarino/pila/issues/1).
Tell us in the report if you'd like to be listed here.
