// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Expo config. `expo prebuild` writes `ios/` and `android/` from it, so
// neither is kept in git. APP_VARIANT picks development or production, and
// signing comes from the git-ignored ios.local.json (see scripts/ios-config.ts).
// The version is MaxHQ's one version, from the root Cargo.toml, and the build
// number is native-build.json's or the release workflow's (see
// scripts/native.ts).

import type { ExpoConfig } from "expo/config";

import { repoVersion } from "../../scripts/release/version.mjs";
import { DEVICE_ENV, iosConfig, readIosLocal, resolveVariant } from "./scripts/ios-config.ts";
import { BUILD_NUMBER_ENV, resolveBuildNumber } from "./scripts/native.ts";
import { withPushExtension } from "./scripts/push-extension.ts";
import { releaseExtra } from "./scripts/release-config.ts";
import nativeBuild from "./native-build.json";

const variant = resolveVariant(process.env.APP_VARIANT);
const ios = iosConfig(variant, readIosLocal(__dirname), process.env[DEVICE_ENV] === "1");

const version = repoVersion();
const buildNumber = String(resolveBuildNumber(nativeBuild, process.env[BUILD_NUMBER_ENV]));

const config: ExpoConfig = {
	name: ios.name,
	slug: "maxhq",
	scheme: ios.scheme,
	version,
	orientation: "portrait",
	userInterfaceStyle: "dark",
	// The desktop app's icon, full bleed: iOS and Android round the corners.
	icon: "./assets/icon.png",
	ios: {
		bundleIdentifier: ios.bundleIdentifier,
		buildNumber,
		supportsTablet: true,
		entitlements: ios.entitlements,
		infoPlist: ios.infoPlist,
		...(ios.appleTeamId ? { appleTeamId: ios.appleTeamId } : {}),
	},
	android: {
		package: ios.bundleIdentifier,
		// The phone's data holds files from your devices, so it stays out of
		// backups, as the core keeps it out of them on iOS.
		allowBackup: false,
		adaptiveIcon: {
			foregroundImage: "./assets/adaptive-icon.png",
			backgroundColor: "#14181F",
		},
	},
	plugins: [
		[
			"expo-camera",
			{
				cameraPermission:
					"Allow MaxHQ to use the camera to scan the pairing code your Mac shows and to take photos you send to an agent.",
				microphonePermission: false,
				recordAudioAndroid: false,
				barcodeScannerEnabled: true,
			},
		],
		// Photos and files you attach to a message. Nothing is recorded.
		[
			"expo-image-picker",
			{
				photosPermission: "Allow MaxHQ to read the photos you choose to send to an agent.",
				cameraPermission:
					"Allow MaxHQ to use the camera to scan the pairing code your Mac shows and to take photos you send to an agent.",
				microphonePermission: false,
			},
		],
		"expo-document-picker",
		// Push needs a signed build, which sets its own aps-environment.
		...(ios.push ? ["expo-notifications"] : []),
	],
	extra: {
		appVariant: variant,
		push: ios.push,
		// Left out when unset, since the manifest turns null into {}.
		...(ios.appGroup ? { appGroup: ios.appGroup } : {}),
		...releaseExtra(process.env),
	},
};

// A signed build gets the extension that shows what each push says.
export default ios.push && ios.appleTeamId
	? withPushExtension(config, {
			bundleIdentifier: `${ios.bundleIdentifier}.push`,
			teamId: ios.appleTeamId,
			keychainGroup: `$(AppIdentifierPrefix)${ios.bundleIdentifier}`,
			version,
			buildNumber,
		})
	: config;
