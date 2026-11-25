// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A config plugin that fits the Android project `expo prebuild` writes to
// MaxHQ's build: the NDK the core is built with, the two ABIs it's built
// for, release builds left unsigned for scripts/android.ts to sign, and the
// phone's data kept out of backups and device-to-device transfer. Each
// change is a plain function of the file's text, so the tests run them
// without a prebuild.

import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import type { ConfigPlugin } from "expo/config-plugins";
import {
	AndroidConfig,
	withAndroidManifest,
	withAppBuildGradle,
	withDangerousMod,
	withGradleProperties,
	withProjectBuildGradle,
} from "expo/config-plugins";

import { ANDROID_ABIS, ANDROID_MIN_SDK, ANDROID_NDK_VERSION } from "./native.ts";

// Firebase's file for Android push. It's git-ignored and stays out until
// the owner adds a Firebase project; the push module's plugin reads it from
// here, and push stays off on Android without it.
export const GOOGLE_SERVICES_FILE = "google-services.json";

/** The app-relative path of google-services.json, or nothing when it's not there. */
export const googleServicesFile = (appDir: string): string | undefined =>
	existsSync(join(appDir, GOOGLE_SERVICES_FILE)) ? `./${GOOGLE_SERVICES_FILE}` : undefined;

// Expo's root plugin sets every module's NDK to the one React Native names
// unless the root project named one first, and gradle.properties can't name
// it, so the root build file does, just before that plugin applies.
const ROOT_PLUGIN = 'apply plugin: "expo-root-project"';

export const pinNdk = (gradle: string): string => {
	if (gradle.includes(`ndkVersion = "${ANDROID_NDK_VERSION}"`)) return gradle;
	if (!gradle.includes(ROOT_PLUGIN)) {
		throw new Error(`The root build.gradle has no ${ROOT_PLUGIN} to pin the NDK before.`);
	}
	return gradle.replace(
		ROOT_PLUGIN,
		`// MaxHQ's core is built with this NDK, so the app links against the same.\next {\n  ndkVersion = "${ANDROID_NDK_VERSION}"\n}\n\n${ROOT_PLUGIN}`
	);
};

// The template signs release builds with the debug key, which no store
// takes; with that line gone Gradle leaves them unsigned.
const RELEASE_SIGNING = /^(\s*release\s*\{[^}]*?)^\s*signingConfig signingConfigs\.debug\s*\n/m;

export const unsignedRelease = (gradle: string): string => {
	if (!RELEASE_SIGNING.test(gradle)) {
		if (/release\s*\{[^}]*signingConfig/.test(gradle)) {
			throw new Error(
				"The app's release build has a signing config this plugin doesn't know."
			);
		}
		return gradle;
	}
	return gradle.replace(RELEASE_SIGNING, "$1");
};

// The two ABIs the core ships for, so the app's other native code builds for
// the same ones, and the oldest Android it runs on.
export const GRADLE_PROPERTIES: Record<string, string> = {
	reactNativeArchitectures: ANDROID_ABIS.join(","),
	"android.minSdkVersion": String(ANDROID_MIN_SDK),
};

type Property = AndroidConfig.Properties.PropertiesItem;

export const withProperties = (items: Property[]): Property[] => {
	const rest = items.filter(
		(item) => item.type !== "property" || !(item.key in GRADLE_PROPERTIES)
	);
	return [
		...rest,
		...Object.entries(GRADLE_PROPERTIES).map(([key, value]): Property => ({
			type: "property",
			key,
			value,
		})),
	];
};

// What Android 12 and later may copy to the cloud or to a new phone: none of
// it. The phone's keys only open on the phone that sealed them, and its data
// holds files from your devices. allowBackup covers the Androids before 12.
const DOMAINS = [
	"root",
	"file",
	"database",
	"sharedpref",
	"external",
	"device_root",
	"device_file",
	"device_database",
	"device_sharedpref",
];
const excluded = DOMAINS.map((domain) => `    <exclude domain="${domain}" path="." />`).join("\n");

export const DATA_EXTRACTION_RULES = `<?xml version="1.0" encoding="utf-8"?>
<data-extraction-rules>
  <cloud-backup>
${excluded}
  </cloud-backup>
  <device-transfer>
${excluded}
  </device-transfer>
</data-extraction-rules>
`;

const RULES_RESOURCE = "data_extraction_rules";

export const withAndroidConfig: ConfigPlugin = (config) => {
	config = withProjectBuildGradle(config, (gradle) => {
		gradle.modResults.contents = pinNdk(gradle.modResults.contents);
		return gradle;
	});
	config = withAppBuildGradle(config, (gradle) => {
		gradle.modResults.contents = unsignedRelease(gradle.modResults.contents);
		return gradle;
	});
	config = withGradleProperties(config, (properties) => {
		properties.modResults = withProperties(properties.modResults);
		return properties;
	});
	config = withDangerousMod(config, [
		"android",
		(mod) => {
			const dir = join(mod.modRequest.platformProjectRoot, "app/src/main/res/xml");
			mkdirSync(dir, { recursive: true });
			writeFileSync(join(dir, `${RULES_RESOURCE}.xml`), DATA_EXTRACTION_RULES);
			return mod;
		},
	]);
	return withAndroidManifest(config, (manifest) => {
		const app = AndroidConfig.Manifest.getMainApplicationOrThrow(manifest.modResults);
		app.$["android:dataExtractionRules"] = `@xml/${RULES_RESOURCE}`;
		return manifest;
	});
};
