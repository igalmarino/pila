---
name: release
description: How to cut a Pila release and maintain its packages (the Arch PKGBUILD, .SRCINFO and AUR; the .deb and .rpm). Use when releasing a version, bumping pkgver or changing packaging/.
---

# Releases

The dev container's git can't push (no credentials): the user pushes `main` from the host,
and the tag and the release are made on GitHub with `gh`, which is logged in.

0. Checks on what's about to be released, which don't run on every push:
   `gh workflow run compat.yml` and `scripts/shells` (Noctalia, Dux Shell), and wait for both
   to pass. Bare-metal numbers come from the user's `scripts/check` on a TTY.
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
   clean systems and attaches them: check that it passes.
4. A commit `pila-git: pkgver for vX` (`PKGBUILD` and `.SRCINFO`). `pkgver` is
   `X.r<commits>.g<7 of the commit's hash>`: of the commit, `vX^{commit}`, not of the annotated
   tag (`git rev-list --count <sha>`).

Pushing that commit starts `aur.yml`, which publishes it to the AUR once the user approves it in
GitHub (the `aur` environment). If the user asks, approve it with
`gh api repos/igalmarino/pila/actions/runs/<run>/pending_deployments` (its environment's id),
then `gh api -X POST …/pending_deployments -F 'environment_ids[]=<id>' -f state=approved`. The
Nix flake (`flake.nix`) takes its version from `Cargo.toml` and needs nothing.

# Arch package

`packaging/aur/pila-git` holds the `PKGBUILD` and `.SRCINFO`; test with `makepkg -f` there.

- Its `package()` runs `scripts/install --no-build` with `PREFIX=/usr DESTDIR=`, so files added
  to `install_files` need no change there. The docs in `share/doc/pila` are installed apart:
  one added for the packages goes in `package()`, `packages.yml`'s Package step and
  `package.nix`'s `postInstall`.
- Keep `depends` in step with the libraries Pila links (`readelf -d`) and README's package
  table.
- Regenerate `.SRCINFO` (`makepkg --printsrcinfo`) after editing the `PKGBUILD`.
- Committing them to `main` starts `aur.yml`: it checks that `.SRCINFO` matches and, once the user
  approves it, publishes them. Its key is the `aur` environment's `AUR_SSH_KEY`. Any change to
  them, a comment too, publishes again.

# Debian and Fedora packages

`packaging/nfpm.yaml` describes the `.deb` and `.rpm`, built by `packages.yml` from what
`scripts/install` stages with `PREFIX=/usr`. A directory added to `install_files` goes in its
`contents`, and a newly linked library in its `depends` (Debian's and Fedora's names), as in the
`PKGBUILD`.
