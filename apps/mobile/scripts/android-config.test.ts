// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
	DATA_EXTRACTION_RULES,
	GRADLE_PROPERTIES,
	googleServicesFile,
	pinNdk,
	unsignedRelease,
	withProperties,
} from "./android-config.ts";

// The parts of the files Expo's Android template writes that the plugin
// changes, as prebuild writes them.
const ROOT_GRADLE = `allprojects {
  repositories {
    google()
  }
}

apply plugin: "expo-root-project"
apply plugin: "com.facebook.react.rootproject"
`;

const APP_GRADLE = `    signingConfigs {
        debug {
            storeFile file('debug.keystore')
            keyAlias 'androiddebugkey'
        }
    }
    buildTypes {
        debug {
            signingConfig signingConfigs.debug
        }
        release {
            // Caution! In production, you need to generate your own keystore file.
            // see https://reactnative.dev/docs/signed-apk-android.
            signingConfig signingConfigs.debug
            def enableShrinkResources = findProperty('android.enableShrinkResourcesInReleaseBuilds') ?: 'false'
            minifyEnabled enableMinifyInReleaseBuilds
        }
    }
`;

describe("pinNdk", () => {
	it("names the core's NDK before Expo's root plugin picks one", () => {
		const pinned = pinNdk(ROOT_GRADLE);
		expect(pinned).toContain('ext {\n  ndkVersion = "30.0.16248370"\n}');
		expect(pinned.indexOf("ndkVersion")).toBeLessThan(pinned.indexOf("expo-root-project"));
		expect(pinNdk(pinned)).toBe(pinned);
	});

	it("stops when the template no longer applies the plugin it pins before", () => {
		expect(() => pinNdk("allprojects {}\n")).toThrow(/expo-root-project/);
	});
});

describe("unsignedRelease", () => {
	it("drops the debug key from release builds only", () => {
		const unsigned = unsignedRelease(APP_GRADLE);
		expect(unsigned.match(/signingConfig signingConfigs\.debug/g)).toHaveLength(1);
		expect(unsigned).toMatch(/debug \{\n\s+signingConfig signingConfigs\.debug\n\s+\}/);
		expect(unsigned).toMatch(
			/signed-apk-android\.\n\s+def enableShrinkResources = findProperty/
		);
		expect(unsignedRelease(unsigned)).toBe(unsigned);
	});

	it("stops at a release signing config it doesn't know", () => {
		const other = APP_GRADLE.replace(
			"            signingConfig signingConfigs.debug\n            def",
			"            signingConfig signingConfigs.upload\n            def"
		);
		expect(() => unsignedRelease(other)).toThrow(/signing config/);
	});
});

describe("withProperties", () => {
	it("builds the app's native code for the core's two ABIs from Android 7.0", () => {
		expect(GRADLE_PROPERTIES).toEqual({
			reactNativeArchitectures: "arm64-v8a,x86_64",
			"android.minSdkVersion": "24",
		});
		const items = withProperties([
			{ type: "comment", value: "Gradle's" },
			{ type: "property", key: "reactNativeArchitectures", value: "armeabi-v7a,arm64-v8a" },
			{ type: "property", key: "hermesEnabled", value: "true" },
		]);
		expect(items).toEqual([
			{ type: "comment", value: "Gradle's" },
			{ type: "property", key: "hermesEnabled", value: "true" },
			{ type: "property", key: "reactNativeArchitectures", value: "arm64-v8a,x86_64" },
			{ type: "property", key: "android.minSdkVersion", value: "24" },
		]);
	});
});

describe("DATA_EXTRACTION_RULES", () => {
	it("keeps every folder out of cloud backups and device-to-device transfer", () => {
		for (const section of ["cloud-backup", "device-transfer"]) {
			const body = DATA_EXTRACTION_RULES.split(`<${section}>`)[1]!.split(`</${section}>`)[0]!;
			for (const domain of [
				"root",
				"file",
				"database",
				"sharedpref",
				"external",
				"device_root",
				"device_file",
				"device_database",
				"device_sharedpref",
			]) {
				expect(body).toContain(`<exclude domain="${domain}" path="." />`);
			}
			expect(body).not.toContain("<include");
		}
	});
});

describe("googleServicesFile", () => {
	it("names Firebase's file only when it's there", () => {
		const dir = mkdtempSync(join(tmpdir(), "maxhq-firebase-"));
		try {
			expect(googleServicesFile(dir)).toBeUndefined();
			writeFileSync(join(dir, "google-services.json"), "{}");
			expect(googleServicesFile(dir)).toBe("./google-services.json");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
