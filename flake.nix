{
  description = "A stacking Wayland compositor built on Smithay";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (
          system: f system nixpkgs.legacyPackages.${system}
        );
    in
    {
      packages = forAllSystems (
        _: pkgs: rec {
          pila = pkgs.callPackage ./package.nix {
            commit = self.shortRev or self.dirtyShortRev or null;
          };
          default = pila;
        }
      );

      devShells = forAllSystems (
        system: pkgs:
        let
          pila = self.packages.${system}.default;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ pila ];
            packages = [
              pkgs.clippy
              pkgs.rustfmt
              pkgs.xwayland
            ];
            inherit (pila.env) RUSTFLAGS;
          };
        }
      );
    };
}
