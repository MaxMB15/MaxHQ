# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Builds maxd from this repository's Cargo workspace. Called through
# pkgsStatic, it makes a static musl binary with rusqlite's bundled SQLite
# compiled in.
{
  lib,
  rustPlatform,
  pname ? "maxd",
}:

let
  root = ../.;
  manifest = lib.importTOML (root + "/Cargo.toml");
in
rustPlatform.buildRustPackage {
  inherit pname;
  inherit (manifest.workspace.package) version;

  # Cargo reads every workspace member's manifest, so the desktop app comes
  # along even though only maxd gets built. maxd embeds the guide.
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/Cargo.toml")
      (root + "/Cargo.lock")
      (root + "/crates")
      (root + "/apps/desktop/src-tauri")
      (root + "/guide")
    ];
  };

  cargoLock.lockFile = root + "/Cargo.lock";
  cargoBuildFlags = [ "-p" "maxd" ];

  # CI and the pre-commit hook run the tests, so the deploy build skips them.
  doCheck = false;

  meta = {
    description = "The MaxHQ daemon, one per device";
    mainProgram = "maxd";
    platforms = lib.platforms.linux;
  };
}
