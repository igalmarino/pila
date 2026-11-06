---
name: release
description: How to cut a pila release and maintain the Arch package (PKGBUILD, .SRCINFO, AUR). Use when releasing a version, bumping pkgver or changing packaging/aur/pila-git.
---

# Releases

0. Compatibility checks on what's about to be released, which don't run on every push:
   `gh workflow run compat.yml`, and wait for it to pass.
1. A commit `Release X` with `CHANGELOG.md`, `Cargo.toml` and `Cargo.lock`.
2. An annotated tag `vX` on it, message `pila X`.
3. A GitHub release with the changelog's section as its text. Publishing it starts `packages.yml`, which
   builds the `.deb` and `.rpm`, installs them on clean systems and attaches them: check that it
   passes.
4. A commit `pila-git: pkgver for vX` (`PKGBUILD` and `.SRCINFO`).

Pushing that commit starts `aur.yml`, which publishes it to the AUR once the user approves it in
GitHub (the `aur` environment). The Nix flake (`flake.nix`) takes its version from
`Cargo.toml` and needs nothing.

# Arch package

`packaging/aur/pila-git` holds the `PKGBUILD` and `.SRCINFO`; test with `makepkg -f` there.

- Its `package()` runs `scripts/install --no-build` with `PREFIX=/usr DESTDIR=`, so files added
  to `install_files` need no change there.
- Keep `depends` in step with the libraries pila links (`readelf -d`) and README's package
  table.
- Regenerate `.SRCINFO` (`makepkg --printsrcinfo`) after editing the `PKGBUILD`.
- Committing them to `main` starts `aur.yml`: it checks that `.SRCINFO` matches and, once the user
  approves it, publishes them. Its key is the `aur` environment's `AUR_SSH_KEY`.
