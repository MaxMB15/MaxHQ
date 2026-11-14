// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// @ts-check
// What counts as native for `pnpm mobile:dev` and the TestFlight check. The
// Rust core is linked into the app, so a change under crates/ needs a new
// native build like any Swift change.
//
// The resolved Expo config is left out, since it holds the Team ID and bundle
// ID from the git-ignored ios.local.json and the build number, which would
// make the hash differ between a Mac and CI and change on every bump. The
// files that write the config, and the icon it names, are hashed instead.
// Package scripts never reach the native project either.

const { DEFAULT_SOURCE_SKIPS, SourceSkips } = require("expo/fingerprint");

const file = (filePath, reason) => ({ type: "file", filePath, reasons: [reason] });

/** @type {import("expo/fingerprint").Config} */
module.exports = {
	platforms: ["ios"],
	sourceSkips:
		DEFAULT_SOURCE_SKIPS | SourceSkips.ExpoConfigAll | SourceSkips.PackageJsonScriptsAll,
	extraSources: [
		{ type: "dir", filePath: "../../crates", reasons: ["rustCore"] },
		file("../../Cargo.toml", "rustCore"),
		file("../../Cargo.lock", "rustCore"),
		file("app.config.ts", "expoConfig"),
		file("scripts/ios-config.ts", "expoConfig"),
		file("scripts/push-extension.ts", "expoConfig"),
		file("assets/icon.png", "appIcon"),
		file("targets/push/NotificationService.swift", "pushExtension"),
		file("targets/push/Sources/PushOpen/PushOpen.swift", "pushExtension"),
	],
};
