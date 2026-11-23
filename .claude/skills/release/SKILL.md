---
name: release
description: How to cut a Pila release and maintain its packages (the Arch PKGBUILD, .SRCINFO and AUR; the .deb and .rpm; the Nix flake). Use when releasing a version, bumping pkgver, changing packaging/, flake.nix or package.nix, adding a file to scripts/install's install_files, or linking a new system library.
---

# Releases

The dev container's git can't push (no credentials): the user pushes `main` from the host,
and the tag and the release are made on GitHub with `gh`, which is logged in.

0. Checks on what's about to be released, which don't run on every push:
   `gh workflow run compat.yml` and `scripts/apps`
   (real apps' windows), and wait for both to pass. Bare-metal numbers come from the user's `scripts/check` on a TTY.
1. A commit `Release X` with `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock` and `fuzz/Cargo.lock`
   (its own lockfile records Pila's version: `cargo metadata --format-version 1 --manifest-path
   fuzz/Cargo.toml >/dev/null` refreshes only that line, where `cargo update -p pila` moves other
   crates too). The user pushes it;
   check it's on GitHub (`gh api repos/igalmarino/pila/commits/main --jq .sha`).
2. An annotated tag `vX` on it, message `pila X`, made through the API (`<sha>` is the
   `Release X` commit):
   `gh api repos/igalmarino/pila/git/tags -f tag=vX -f message="pila X" -f object=<sha> -f type=commit --jq .sha`,
   then `gh api repos/igalmarino/pila/git/refs -f ref=refs/tags/vX -f sha=<the tag object's sha>`.
   The tag must be `v` plus Cargo.toml's version: `packages.yml` checks.
3. A GitHub release with the changelog's section as its text:
   `awk '/^## X /{on=1;next} /^## /{if(on)exit} on' CHANGELOG.md > notes.md` (in the
   scratchpad), then `gh release create vX --verify-tag --title "pila X" --notes-file notes.md`.
   Publishing it starts `packages.yml`, which builds the `.deb` and `.rpm`, installs them on
   clean systems and attaches them: check that it passes. Its success starts `repos.yml`, which
   rebuilds the APT and DNF repositories and installs the new version from them on clean
   systems: check that it passes too (its `verify` jobs retry while Pages' cache is stale).
4. A commit `pila-git: pkgver for vX` (`PKGBUILD` and `.SRCINFO`). `pkgver` is
   `X.r<commits>.g<7 of the commit's hash>`: of the commit, `vX^{commit}`, not of the annotated
   tag (`git rev-list --count <sha>`).

Pushing that commit starts `aur.yml`, which waits for an approval in GitHub (the `aur`
environment) before it publishes to the AUR. For a release's `pkgver` commit, approve it as part
of the release, without asking (the user's standing instruction since 2026-11-14): once its
`.SRCINFO` check passed and the run waits, `gh api
repos/igalmarino/pila/actions/runs/<run>/pending_deployments` (its environment's id), then
`gh api -X POST …/pending_deployments -F 'environment_ids[]=<id>' -f state=approved`, and check
that the AUR shows the new `pkgver` (`https://aur.archlinux.org/rpc/v5/info?arg[]=pila-git`).
Any other change to the `PKGBUILD` that waits there is still the user's to approve. The Nix
flake (`flake.nix`) takes its version from `Cargo.toml` and needs nothing.

# pila-desktop

A metapackage with no files: `pila` plus the standard tools `config.example.toml`'s defaults
run (terminal, bar, launcher, notifications, idle and lock, the keys' programs), so `pila` alone
stays the compositor alone. Its list is in four places that change together when a default tool
does: `packaging/aur/pila-desktop/PKGBUILD` (raise its `pkgver`), `packaging/nfpm-desktop.yaml`
(built and installed with its tools by `packages.yml`, with the release's version),
`flake.nix`'s `pila-desktop` (built by `compat.yml`) and docs/INSTALL.md's "Packages for a full
session". A release needs nothing else for it.

# Arch package

`packaging/aur/pila-git` holds the `PKGBUILD` and `.SRCINFO`; test with `makepkg -f` there.
`aur.yml` publishes every directory in `packaging/aur` (`pila-desktop` too) under one approval.

- Its `package()` runs `scripts/install --no-build` with `PREFIX=/usr DESTDIR=`, so files added
  to `install_files` need no change there. The docs in `share/doc/pila` are installed apart:
  one added for the packages goes in `package()`, `packages.yml`'s Package step,
  `ci.yml`'s Packages build and `package.nix`'s `postInstall`.
- Keep `depends` in step with the libraries Pila links (`readelf -d`) and docs/INSTALL.md's package
  table.
- Regenerate `.SRCINFO` (`makepkg --printsrcinfo`) after editing the `PKGBUILD`.
- Committing them to `main` starts `aur.yml`: it checks that `.SRCINFO` matches and, once the user
  approves it, publishes them. Its key is the `aur` environment's `AUR_SSH_KEY`. Any change to
  them, a comment too, publishes again.

# Debian and Fedora packages

`packaging/nfpm.yaml` describes the `.deb` and `.rpm`, built by `packages.yml` from what
`scripts/install` stages with `PREFIX=/usr`, and by CI's Packages build at every push (so a
mistake shows before a release; nfpm's version and checksum are in both workflows). A directory added to `install_files` goes in its
`contents`, and a newly linked library in its `depends` (Debian's and Fedora's names), as in the
`PKGBUILD`.

The `.deb` is built on the oldest Ubuntu supported (24.04) and the `.rpm` on the oldest Fedora
maintained (`fedora:44`), so one package serves every release after it; each requires its build
system's glibc (`libc6 (>= 2.39)`, `glibc >= 2.43`). When that Fedora reaches its end of life
(44: 2027-06-02, `https://endoflife.date/api/fedora.json`), move `packages.yml`'s rpm build image,
its install test, `repos.yml`'s `verify` image and the `glibc` line to the next one, and the
docs/INSTALL.md's "Fedora 44 or newer".

# APT and DNF repositories

`repos.yml` (after a release's `packages.yml`, or `gh workflow run repos.yml`) downloads the
`.deb`, `.rpm` and `SHA256SUMS` of the last three releases, signs the `.rpm`s and the metadata
(`apt-ftparchive`, `createrepo_c`) and deploys the whole site to GitHub Pages
(`https://igalmarino.github.io/pila/`: `deb/`, `rpm/`, `pila.asc`, `index.html` from
`packaging/repo-index.html`); nothing of it is in git. The key is ed25519,
`67A0F1D196AEC2CBB1560ECA2413E5C5541073F3`, expiring 2031-11-17: its private half is only the
`repos` environment's secret `REPO_SIGNING_KEY` (and the user's backup), its public half
`packaging/pila.asc`, which the workflow checks it matches. Extending or replacing it changes
`packaging/pila.asc`, the secret, and the fingerprint in docs/INSTALL.md and `packaging/repo-index.html`.

# Nix package

`flake.nix` and `package.nix` install the same files as `scripts/install`: a file added to
`install_files` is added there too. A dependency on a new system library goes in
`package.nix`'s `buildInputs`. `flake.lock` pins nixpkgs. There's no Nix in the dev container:
`compat.yml`'s Nix job builds the package and starts Pila from it.
