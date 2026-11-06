# pila as a Nix package: what scripts/install installs, from the flake
# (flake.nix). pila loads libEGL and libwayland-client at runtime, so they're
# linked by force to be found outside the standard library paths, as nixpkgs
# does for niri.
{
  lib,
  rustPlatform,
  makeWrapper,
  pkg-config,
  libgbm,
  libglvnd,
  libinput,
  libxkbcommon,
  seatd,
  systemdLibs,
  wayland,
  xwayland,
  # The commit it's built from, for `pila --version`.
  commit ? null,
}:

let
  rustFlags = toString (
    map (arg: "-C link-arg=" + arg) [
      "-Wl,--push-state,--no-as-needed"
      "-lEGL"
      "-lwayland-client"
      "-Wl,--pop-state"
    ]
  );
in
rustPlatform.buildRustPackage {
  pname = "pila";
  version = (lib.importTOML ./Cargo.toml).package.version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.lock
      ./Cargo.toml
      ./README.md
      ./build.rs
      ./config.example.toml
      ./resources
      ./scripts/report
      ./src
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [
    makeWrapper
    pkg-config
  ];

  buildInputs = [
    libgbm
    libglvnd
    libinput
    libxkbcommon
    seatd
    systemdLibs
    wayland
  ];

  env = {
    RUSTFLAGS = rustFlags;
  }
  // lib.optionalAttrs (commit != null) { PILA_BUILD_COMMIT = commit; };

  # The integration tests need a GPU or a software renderer's vendor files, and
  # Xwayland: CI runs them outside Nix's sandbox.
  doCheck = false;

  postInstall = ''
    install -Dm755 resources/session/pila-session $out/bin/pila-session
    install -Dm755 scripts/report $out/bin/pila-report
    install -Dm644 resources/session/pila.desktop $out/share/wayland-sessions/pila.desktop
    substituteInPlace $out/share/wayland-sessions/pila.desktop \
      --replace-fail '@BINDIR@' "$out/bin"
    install -Dm644 resources/xdg/xdg-desktop-portal/pila-portals.conf \
      -t $out/share/xdg-desktop-portal
    install -Dm644 resources/xdg/xdg-desktop-portal-wlr/pila \
      -t $out/etc/xdg/xdg-desktop-portal-wlr
    install -Dm644 README.md config.example.toml -t $out/share/doc/pila

    # After the user's own, if they have one.
    wrapProgram $out/bin/pila --suffix PATH : ${lib.makeBinPath [ xwayland ]}
  '';

  passthru = {
    # For `services.displayManager.sessionPackages`.
    providedSessions = [ "pila" ];
    # For the flake's devShell.
    inherit rustFlags;
  };

  meta = {
    description = "A stacking Wayland compositor built on Smithay";
    homepage = "https://github.com/igalmarino/pila";
    license = lib.licenses.gpl3Plus;
    mainProgram = "pila";
    platforms = lib.platforms.linux;
  };
}
