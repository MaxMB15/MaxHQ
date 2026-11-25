// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Android steps behind `pnpm mobile:android:dev`, `mobile:android:apk`
// and `mobile:android:aab`, and the release workflow's `android-release` and
// `android-sign`, as plain lists of commands like native.ts's, so the tests
// and --dry-run read them without running anything. scripts/mobile.ts runs
// them.
//
// Gradle builds the app from the project `expo prebuild` writes in
// android/. A dev build is signed with the debug key prebuild puts there.
// A release build leaves Gradle unsigned (see android-config.ts) and is
// signed after it with the upload key the ANDROID_KEY* variables hold, so
// neither the key nor its passwords reach Gradle or the plugins it runs.

import { ANDROID_NDK_VERSION, expo, expoEnv, type Step } from "./native.ts";

type Variant = "development" | "production";

/** A release build: the APK, the App Bundle, or both from one Gradle run. */
export type ReleaseKind = "apk" | "aab" | "both";

// What Gradle writes, from the app's folder.
export const DEBUG_APK = "android/app/build/outputs/apk/debug/app-debug.apk";
export const UNSIGNED_APK = "android/app/build/outputs/apk/release/app-release-unsigned.apk";
const ALIGNED_APK = "android/app/build/outputs/apk/release/app-release-aligned.apk";
export const RELEASE_APK = "android/app/build/outputs/apk/release/app-release.apk";
export const RELEASE_AAB = "android/app/build/outputs/bundle/release/app-release.aab";

const prebuild = (variant: Variant) =>
	expo(["prebuild", "--clean", "--platform", "android"], expoEnv(variant, false));

// Gradle reads the app's config again as it builds, so it gets the same
// variant prebuild had. Only the app's project builds: its tasks build the
// modules it links, and the modules' own outputs aren't needed.
const gradle = (tasks: string[], variant: Variant): Step => ({
	cmd: "./gradlew",
	args: tasks.map((task) => `:app:${task}`),
	env: expoEnv(variant, false),
	cwd: "android",
});

const RELEASE_TASKS: Record<ReleaseKind, string[]> = {
	apk: ["assembleRelease"],
	aab: ["bundleRelease"],
	both: ["assembleRelease", "bundleRelease"],
};

/**
 * The debug APK of the development variant, signed with the debug key. It
 * loads its JS from Metro, as the iOS dev build does.
 */
export const devApkSteps = (): Step[] => [
	prebuild("development"),
	gradle(["assembleDebug"], "development"),
];

/**
 * The production app as an APK, an App Bundle or both, unsigned. Both build
 * in one Gradle run, so the native code compiles once for the two.
 */
export const releaseSteps = (kind: ReleaseKind): Step[] => [
	prebuild("production"),
	gradle(RELEASE_TASKS[kind], "production"),
];

/** Where an unsigned release build lands, and where its signed copy goes. */
export const releaseOutput = (kind: "apk" | "aab", signed: boolean) =>
	kind === "aab" ? RELEASE_AAB : signed ? RELEASE_APK : UNSIGNED_APK;

// The upload key, as the release workflow's secrets hold it: the keystore
// file in base64, its password, the key's alias and the key's password.
export const SIGNING_ENV = {
	keystore: "ANDROID_KEYSTORE_BASE64",
	storePassword: "ANDROID_KEYSTORE_PASSWORD",
	alias: "ANDROID_KEY_ALIAS",
	keyPassword: "ANDROID_KEY_PASSWORD",
} as const;

export type Signing = {
	keystore: Buffer;
	storePassword: string;
	alias: string;
	keyPassword: string;
};

/**
 * The key the environment holds, or null when it holds none. Half a key is
 * an error that names what's missing and never echoes what's set.
 */
export const signingFromEnv = (env: Record<string, string | undefined>): Signing | null => {
	const names = Object.values(SIGNING_ENV);
	const missing = names.filter((name) => !env[name]);
	if (missing.length === names.length) return null;
	if (missing.length > 0) {
		throw new Error(`Signing needs ${names.join(", ")}; ${missing.join(", ")} isn't set.`);
	}
	const text = env[SIGNING_ENV.keystore]!.replace(/\s/g, "");
	if (!/^[A-Za-z0-9+/]+={0,2}$/.test(text) || text.length % 4 !== 0) {
		throw new Error(`${SIGNING_ENV.keystore} isn't base64.`);
	}
	return {
		keystore: Buffer.from(text, "base64"),
		storePassword: env[SIGNING_ENV.storePassword]!,
		alias: env[SIGNING_ENV.alias]!,
		keyPassword: env[SIGNING_ENV.keyPassword]!,
	};
};

/**
 * The files scripts/mobile.ts writes the key into for one signing, in a
 * folder only this user can read, and removes after. The signers read the
 * passwords from them, so no password is on a command line.
 */
export const signingFiles = (dir: string) => ({
	keystore: `${dir}/upload.keystore`,
	storePassword: `${dir}/store-password`,
	keyPassword: `${dir}/key-password`,
});

export type SigningFiles = ReturnType<typeof signingFiles>;

/**
 * Signs the unsigned APK into RELEASE_APK. zipalign lines its native
 * libraries up on 16 KB pages first, as Android 15 asks, since apksigner
 * signs what it's given as it is. `buildTools` is the SDK's build-tools
 * folder.
 */
export const signApkSteps = (buildTools: string, files: SigningFiles, alias: string): Step[] => [
	{
		cmd: `${buildTools}/zipalign`,
		args: ["-P", "16", "-f", "4", UNSIGNED_APK, ALIGNED_APK],
		cwd: "app",
	},
	{
		cmd: `${buildTools}/apksigner`,
		args: [
			"sign",
			"--ks",
			files.keystore,
			"--ks-key-alias",
			alias,
			"--ks-pass",
			`file:${files.storePassword}`,
			"--key-pass",
			`file:${files.keyPassword}`,
			"--out",
			RELEASE_APK,
			ALIGNED_APK,
		],
		cwd: "app",
	},
	{ cmd: `${buildTools}/apksigner`, args: ["verify", RELEASE_APK], cwd: "app" },
];

/**
 * Signs the App Bundle in place. Play takes a bundle signed with the upload
 * key the way a jar is, which jarsigner does; `jarsigner` is the JDK's.
 */
export const signAabSteps = (jarsigner: string, files: SigningFiles, alias: string): Step[] => [
	{
		cmd: jarsigner,
		args: [
			"-keystore",
			files.keystore,
			"-storepass:file",
			files.storePassword,
			"-keypass:file",
			files.keyPassword,
			RELEASE_AAB,
			alias,
		],
		cwd: "app",
	},
	{ cmd: jarsigner, args: ["-verify", RELEASE_AAB], cwd: "app" },
];

export const NO_SDK =
	"Set ANDROID_HOME to the Android SDK, such as the one Android Studio or the android-commandlinetools cask installs.";

/**
 * What the Android builds need from the environment beyond what's set: the
 * pinned NDK for cargo-ndk, in place of any other ANDROID_NDK_HOME names, so
 * the core is built with the NDK Gradle links it with. Gradle reads
 * ANDROID_HOME itself.
 */
export const androidEnv = (
	env: Record<string, string | undefined>
): { sdk: string; add: Record<string, string> } => {
	const sdk = env.ANDROID_HOME || env.ANDROID_SDK_ROOT;
	if (!sdk) throw new Error(NO_SDK);
	return { sdk, add: { ANDROID_NDK_HOME: `${sdk}/ndk/${ANDROID_NDK_VERSION}` } };
};

/** The newest build-tools version among the SDK's, or none. */
export const newestBuildTools = (versions: string[]): string | undefined =>
	versions
		.filter((version) => /^\d+(\.\d+)*$/.test(version))
		.sort((a, b) => {
			const [x, y] = [a, b].map((v) => v.split(".").map(Number));
			for (let i = 0; i < Math.max(x!.length, y!.length); i++) {
				const d = (x![i] ?? 0) - (y![i] ?? 0);
				if (d !== 0) return d;
			}
			return 0;
		})
		.at(-1);

/**
 * The serial of the only phone `adb devices -l` lists as ready, or nothing
 * when there are none or several. Emulators never count, so a dev install
 * never lands on one another tool leased.
 */
export const onlyAndroidPhone = (listing: string): string | undefined => {
	const phones = listing
		.split("\n")
		.slice(1)
		.map((row) => row.trim().split(/\s+/))
		.filter(
			([serial, state, ...rest]) =>
				serial !== undefined &&
				state === "device" &&
				!serial.startsWith("emulator-") &&
				!rest.some((field) => field.startsWith("product:sdk_"))
		);
	return phones.length === 1 ? phones[0]![0] : undefined;
};

/** The dev APK onto the phone `serial` names, in place of the one there. */
export const devInstallStep = (serial: string): Step => ({
	cmd: "adb",
	args: ["-s", serial, "install", "-r", DEBUG_APK],
	cwd: "app",
});
