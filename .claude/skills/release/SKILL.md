---
name: release
description: How to cut a pila release and maintain the Arch package (PKGBUILD, .SRCINFO, AUR). Use when releasing a version, bumping pkgver or changing packaging/aur/pila-git.
---

# Releases

1. A commit `Release X` with `CHANGELOG.md`, `Cargo.toml` and `Cargo.lock`.
2. An annotated tag `vX` on it, message `pila X`.
3. A GitHub release with the changelog's section as its text.
4. A commit `pila-git: pkgver for vX` (`PKGBUILD` and `.SRCINFO`).

The user publishes it to the AUR.

# Arch package

`packaging/aur/pila-git` holds the `PKGBUILD` and `.SRCINFO`; test with `makepkg -f` there.

- Its `package()` runs `scripts/install --no-build` with `PREFIX=/usr DESTDIR=`, so files added
  to `install_files` need no change there.
- Keep `depends` in step with the libraries pila links (`readelf -d`) and README's package
  table.
- Regenerate `.SRCINFO` (`makepkg --printsrcinfo`) after editing the `PKGBUILD`.

The user publishes it to the AUR.
