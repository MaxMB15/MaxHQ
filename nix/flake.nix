# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# The flake for running maxd on NixOS. It lives in nix/, so a system flake
# takes it from this repository's URL with `?dir=nix`.
{
  description = "maxd, the MaxHQ daemon, for NixOS";

  # The nixpkgs devbox runs, so a build there shares its packages. A
  # system flake can make this follow its own nixpkgs.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/567a49d1913ce81ac6e9582e3553dd90a955875f";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forEachSystem (
        pkgs:
        rec {
          maxd = pkgs.callPackage ./package.nix { };
          default = maxd;
        }
        # The guest binary the host pushes into each container workspace.
        # The containers run Debian, which has no Nix store to load a Nix
        # binary from, so this one is static, against musl.
        // nixpkgs.lib.optionalAttrs (pkgs.stdenv.hostPlatform.system == "x86_64-linux") {
          maxd-guest = pkgs.pkgsStatic.callPackage ./package.nix { pname = "maxd-guest"; };
        }
      );

      # The NixOS VM test of services.maxd.host. It boots a VM, so it runs
      # where KVM does, such as the home server's Linux test run.
      checks = forEachSystem (pkgs: {
        host = pkgs.testers.runNixOSTest ./tests/host.nix;
      });

      nixosModules.maxd = ./module.nix;
      nixosModules.default = self.nixosModules.maxd;
    };
}
