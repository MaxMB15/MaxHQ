// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	androidEnv,
	DEBUG_APK,
	devApkSteps,
	devInstallStep,
	newestBuildTools,
	NO_SDK,
	onlyAndroidPhone,
	RELEASE_AAB,
	RELEASE_APK,
	releaseOutput,
	releaseSteps,
	signAabSteps,
	signApkSteps,
	SIGNING_ENV,
	signingFiles,
	signingFromEnv,
	UNSIGNED_APK,
} from "./android.ts";

const line = (step: { cmd: string; args: string[] }) => [step.cmd, ...step.args].join(" ");

describe("devApkSteps", () => {
	it("prebuilds the development app from clean and builds the debug APK", () => {
		const steps = devApkSteps();
		expect(steps.map(line)).toEqual([
			"expo prebuild --clean --platform android",
			"./gradlew :app:assembleDebug",
		]);
		for (const step of steps) {
			expect(step.env).toEqual({ APP_VARIANT: "development", EXPO_NO_GIT_STATUS: "1" });
		}
		expect(steps.map((step) => step.cwd)).toEqual(["app", "android"]);
		expect(DEBUG_APK).toBe("android/app/build/outputs/apk/debug/app-debug.apk");
	});
});

describe("releaseSteps", () => {
	it("prebuilds production and builds an APK or an App Bundle", () => {
		for (const [kind, task] of [
			["apk", "assembleRelease"],
			["aab", "bundleRelease"],
		] as const) {
			const steps = releaseSteps(kind);
			expect(steps.map(line)).toEqual([
				"expo prebuild --clean --platform android",
				`./gradlew :app:${task}`,
			]);
			for (const step of steps) expect(step.env!.APP_VARIANT).toBe("production");
		}
	});

	it("names where each build lands", () => {
		expect(releaseOutput("apk", false)).toBe(UNSIGNED_APK);
		expect(releaseOutput("apk", true)).toBe(RELEASE_APK);
		expect(releaseOutput("aab", false)).toBe(RELEASE_AAB);
		expect(releaseOutput("aab", true)).toBe(RELEASE_AAB);
	});
});

describe("signingFromEnv", () => {
	const keystore = Buffer.from("not a real keystore");
	const full = {
		[SIGNING_ENV.keystore]: keystore.toString("base64"),
		[SIGNING_ENV.storePassword]: "store-secret",
		[SIGNING_ENV.alias]: "upload",
		[SIGNING_ENV.keyPassword]: "key-secret",
	};

	it("names the four variables the release workflow sets", () => {
		expect(Object.values(SIGNING_ENV)).toEqual([
			"ANDROID_KEYSTORE_BASE64",
			"ANDROID_KEYSTORE_PASSWORD",
			"ANDROID_KEY_ALIAS",
			"ANDROID_KEY_PASSWORD",
		]);
	});

	it("reads all four or none", () => {
		expect(signingFromEnv({})).toBeNull();
		expect(signingFromEnv({ [SIGNING_ENV.alias]: "" })).toBeNull();
		expect(signingFromEnv(full)).toEqual({
			keystore,
			storePassword: "store-secret",
			alias: "upload",
			keyPassword: "key-secret",
		});
	});

	it("takes base64 wrapped over lines, as base64 writes it", () => {
		const wrapped = full[SIGNING_ENV.keystore].replace(/(.{8})/g, "$1\n");
		expect(signingFromEnv({ ...full, [SIGNING_ENV.keystore]: wrapped })!.keystore).toEqual(
			keystore
		);
	});

	it("names what's missing from half a key and never what's set", () => {
		const half = { ...full, [SIGNING_ENV.keyPassword]: undefined };
		expect(() => signingFromEnv(half)).toThrow(/; ANDROID_KEY_PASSWORD isn't set/);
		expect(() => signingFromEnv(half)).not.toThrow(/store-secret|upload|key-secret/);
	});

	it("refuses a keystore that isn't base64", () => {
		expect(() => signingFromEnv({ ...full, [SIGNING_ENV.keystore]: "not base64!" })).toThrow(
			/ANDROID_KEYSTORE_BASE64 isn't base64/
		);
	});
});

describe("signing steps", () => {
	const files = signingFiles("/keys");

	it("keeps the key and its passwords in files, not on the command line", () => {
		expect(files).toEqual({
			keystore: "/keys/upload.keystore",
			storePassword: "/keys/store-password",
			keyPassword: "/keys/key-password",
		});
		const steps = [
			...signApkSteps("/sdk/build-tools/36.0.0", files, "upload"),
			...signAabSteps("/jdk/bin/jarsigner", files, "upload"),
		];
		for (const step of steps) {
			expect(step.args.join(" ")).not.toMatch(/secret/);
			expect(step.cwd).toBe("app");
		}
	});

	it("aligns the APK on 16 KB pages, signs it with apksigner and checks it", () => {
		expect(signApkSteps("/bt", files, "upload").map(line)).toEqual([
			`/bt/zipalign -P 16 -f 4 ${UNSIGNED_APK} android/app/build/outputs/apk/release/app-release-aligned.apk`,
			`/bt/apksigner sign --ks /keys/upload.keystore --ks-key-alias upload --ks-pass file:/keys/store-password --key-pass file:/keys/key-password --out ${RELEASE_APK} android/app/build/outputs/apk/release/app-release-aligned.apk`,
			`/bt/apksigner verify ${RELEASE_APK}`,
		]);
	});

	it("signs the App Bundle with jarsigner and checks it", () => {
		expect(signAabSteps("jarsigner", files, "upload").map(line)).toEqual([
			`jarsigner -keystore /keys/upload.keystore -storepass:file /keys/store-password -keypass:file /keys/key-password ${RELEASE_AAB} upload`,
			`jarsigner -verify ${RELEASE_AAB}`,
		]);
	});
});

describe("androidEnv", () => {
	it("points cargo-ndk at the pinned NDK in the SDK", () => {
		expect(androidEnv({ ANDROID_HOME: "/sdk" })).toEqual({
			sdk: "/sdk",
			add: { ANDROID_NDK_HOME: "/sdk/ndk/30.0.16248370" },
		});
		expect(androidEnv({ ANDROID_SDK_ROOT: "/old" }).sdk).toBe("/old");
	});

	it("names the pinned NDK over one that's named already", () => {
		expect(androidEnv({ ANDROID_HOME: "/sdk", ANDROID_NDK_HOME: "/sdk/ndk/27.1" }).add).toEqual(
			{
				ANDROID_NDK_HOME: "/sdk/ndk/30.0.16248370",
			}
		);
	});

	it("says where to point it without an SDK", () => {
		expect(() => androidEnv({})).toThrow(NO_SDK);
	});
});

describe("newestBuildTools", () => {
	it("picks the highest version and skips what isn't one", () => {
		expect(newestBuildTools(["35.0.0", "36.0.0", "9.0.0", "36.0.0-rc1", ".DS_Store"])).toBe(
			"36.0.0"
		);
		expect(newestBuildTools(["34.0.0", "34.0.10", "34.0.9"])).toBe("34.0.10");
		expect(newestBuildTools([])).toBeUndefined();
	});
});

describe("onlyAndroidPhone", () => {
	const header = "List of devices attached";

	it("names the one phone that's ready", () => {
		const listing = [
			header,
			"R5CT1234ABC            device usb:1-1 product:a55xnsxx model:SM_A556B device:a55x transport_id:3",
			"",
		].join("\n");
		expect(onlyAndroidPhone(listing)).toBe("R5CT1234ABC");
		expect(devInstallStep("R5CT1234ABC")).toEqual({
			cmd: "adb",
			args: ["-s", "R5CT1234ABC", "install", "-r", DEBUG_APK],
			cwd: "app",
		});
	});

	it("never counts an emulator, so a leased one is left alone", () => {
		const listing = [
			header,
			"emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 transport_id:1",
			"localhost:5555         device product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 transport_id:2",
		].join("\n");
		expect(onlyAndroidPhone(listing)).toBeUndefined();
	});

	it("names none when phones aren't ready or there are several", () => {
		expect(onlyAndroidPhone(header)).toBeUndefined();
		expect(onlyAndroidPhone(`${header}\nR5CT1234ABC unauthorized usb:1-1 transport_id:3`)).toBe(
			undefined
		);
		expect(
			onlyAndroidPhone(
				`${header}\nR5CT1234ABC device usb:1-1 transport_id:3\n0A1B2C3D device usb:1-2 transport_id:4`
			)
		).toBeUndefined();
	});
});
