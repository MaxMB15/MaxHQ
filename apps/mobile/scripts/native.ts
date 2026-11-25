// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The steps behind `pnpm mobile:core`, `mobile:dev`, `mobile:testflight` and
// `mobile:install`, and the Android core's, as plain lists of commands so the
// tests and --dry-run can read them without running anything.
// scripts/mobile.ts runs them; scripts/android.ts holds the rest of
// Android's.

import { iosConfig, type IosLocal } from "./ios-config.ts";

export type Step = {
	cmd: string;
	args: string[];
	env?: Record<string, string>;
	cwd?: "repo" | "app" | "core" | "android";
	// Values in `args` the printed command shows as ***.
	hide?: string[];
};

// The two Apple targets the core is built for.
export const RUST_TARGETS = ["aarch64-apple-ios", "aarch64-apple-ios-sim"] as const;

// The Android ABIs the core is built for: phones, and the emulator on an
// x86_64 PC. Apple silicon's emulator runs arm64-v8a. The module's Gradle
// build packs the same two.
export const ANDROID_ABIS = ["arm64-v8a", "x86_64"] as const;

// The oldest Android the app runs on, Android 7.0, as React Native's own.
export const ANDROID_MIN_SDK = 24;

// The one NDK the core and the app's C++ build with, so cargo-ndk's Rust
// and Gradle's CMake link against the same runtime. android-config.ts pins
// Gradle to it.
export const ANDROID_NDK_VERSION = "30.0.16248370";

export type CorePlatform = "ios" | "android";

export const NATIVE_BUILD_FILE = "apps/mobile/native-build.json";
export const DEV_BUILD_FILE = "apps/mobile/.expo/maxhq-dev-build.json";

// Where a phone's dev build goes, outside ios/ so a clean prebuild keeps
// Xcode's cache.
export const DEV_DERIVED_DATA = ".expo/xcode";

// The core's React Native module, which uniffi-bindgen-react-native writes.
// It runs in the module's folder, since it reads the package.json there.
export const CORE_MODULE = "modules/maxhq-core";

// The cargo command that builds the core for one target. Android's goes
// through cargo-ndk, which finds the NDK's compiler for the ABI and the API
// level.
const cargoBuild = (platform: CorePlatform, target: string) =>
	platform === "ios"
		? ["build", "-p", "maxhq-mobile", "--target", target]
		: [
				"ndk",
				"--platform",
				String(ANDROID_MIN_SDK),
				"--target",
				target,
				"build",
				"-p",
				"maxhq-mobile",
			];

// Debug for dev builds, release with thin LTO for TestFlight and the Play
// Store. Cargo builds each target, then ubrn writes the bindings and the
// xcframework, or the libraries Gradle links, from those builds.
export const coreSteps = (release: boolean, platform: CorePlatform = "ios"): Step[] => [
	...(platform === "ios" ? RUST_TARGETS : ANDROID_ABIS).map((target): Step => ({
		cmd: "cargo",
		args: [...cargoBuild(platform, target), ...(release ? ["--release"] : [])],
		...(release ? { env: { CARGO_PROFILE_RELEASE_LTO: "thin" } } : {}),
		cwd: "repo",
	})),
	{
		cmd: "ubrn",
		args: [
			"build",
			platform,
			"--config",
			"ubrn.config.yaml",
			"--no-cargo",
			"--and-generate",
			...(release ? ["--release"] : []),
		],
		cwd: "core",
	},
];

export const expoEnv = (variant: "development" | "production", device: boolean) => ({
	APP_VARIANT: variant,
	EXPO_NO_GIT_STATUS: "1",
	...(device ? { MAXHQ_IOS_DEVICE: "1" } : {}),
});

export const expo = (args: string[], env: Record<string, string>): Step => ({
	cmd: "expo",
	args,
	env,
	cwd: "app",
});

/** A device as `xcrun devicectl list devices --json-output` lists it. */
export type ListedDevice = {
	hardwareProperties?: { platform?: string; reality?: string; udid?: string };
	connectionProperties?: { pairingState?: string };
};

/** The UDID of the only paired iPhone, or nothing when there are none or several. */
export const onlyPhone = (devices: ListedDevice[]): string | undefined => {
	const phones = devices.filter(
		({ hardwareProperties: hw, connectionProperties: link }) =>
			hw?.platform === "iOS" &&
			hw.reality === "physical" &&
			hw.udid !== undefined &&
			link?.pairingState === "paired"
	);
	return phones.length === 1 ? phones[0]!.hardwareProperties!.udid : undefined;
};

export const NO_PHONE =
	"Found no paired iPhone, or more than one. Pair the phone with this Mac in Xcode's Devices and Simulators window, then run this again.";

// expo run:ios hangs connecting to a phone reached over Wi-Fi, so devicectl
// installs instead, over Wi-Fi or USB alike. It needs the phone's UDID, so a
// build for a phone stops before it starts without one.
const phoneInstall = (phone: string | undefined, app: string): Step => {
	if (phone === undefined) throw new Error(NO_PHONE);
	return {
		cmd: "xcrun",
		args: ["devicectl", "device", "install", "app", "--device", phone, app],
		cwd: "app",
	};
};

// A new native build goes to the phone (or the simulator) without Metro, so
// the script can note its fingerprint before Metro takes over the terminal.
// Prebuild starts clean, so a production prebuild left in
// ios/ never leaks into a dev build.
//
// Expo leaves -allowProvisioningUpdates out once the project names a team,
// as ours does, and Xcode then signs with the team's wildcard profile, which
// can't carry push or the app group. So a phone's build runs here with it,
// as the TestFlight archive does, and devicectl installs it.
/**
 * The steps for a dev build, with a phone's install on its own so Metro can
 * start even when the phone doesn't take it. `phone` names the iPhone. The
 * simulator's build installs itself.
 */
export const devBuildSteps = (
	device: boolean,
	phone?: string
): { build: Step[]; install?: Step } => {
	const env = expoEnv("development", device);
	const prebuild = expo(["prebuild", "--clean", "--platform", "ios"], env);
	if (!device) return { build: [prebuild, expo(["run:ios", "--no-bundler"], env)] };
	const name = projectName(iosConfig("development", null, false).name);
	return {
		build: [
			prebuild,
			{
				cmd: "xcodebuild",
				args: [
					"-workspace",
					`ios/${name}.xcworkspace`,
					"-scheme",
					name,
					"-configuration",
					"Debug",
					"-destination",
					"generic/platform=iOS",
					"-derivedDataPath",
					DEV_DERIVED_DATA,
					"-allowProvisioningUpdates",
					"-allowProvisioningDeviceRegistration",
					"build",
				],
				// As Expo builds it: the build never starts Metro itself.
				env: { ...env, RCT_NO_LAUNCH_PACKAGER: "true" },
				cwd: "app",
			},
		],
		install: phoneInstall(
			phone,
			`${DEV_DERIVED_DATA}/Build/Products/Debug-iphoneos/${name}.app`
		),
	};
};

export const metroStep = (): Step =>
	expo(["start", "--dev-client"], { APP_VARIANT: "development" });

// Expo names the Xcode workspace and scheme after the app with anything but
// letters and digits taken out.
const projectName = (name: string) => name.replace(/[^A-Za-z0-9]/g, "");

// A flat plist of strings and booleans, which is all the export options need.
const plist = (entries: Record<string, string | boolean>) => `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
${Object.entries(entries)
	.map(([key, value]) => {
		const text = typeof value === "string" ? `<string>${value}</string>` : `<${value}/>`;
		return `\t<key>${key}</key>\n\t${text}\n`;
	})
	.join("")}</dict>
</plist>
`;

// How an archive leaves the Mac. TestFlight's goes straight to App Store
// Connect. The release workflow's is an .ipa, which its upload job exports
// and uploads once every build of the release has passed. An
// install's is an .ipa signed ad hoc for the phones registered with the
// team, which keeps the production aps-environment, so the app gets
// production pushes.
const EXPORTS = {
	testflight: {
		method: "app-store-connect",
		destination: "upload",
		manageAppVersionAndBuildNumber: false,
	},
	ipa: {
		method: "app-store-connect",
		destination: "export",
		manageAppVersionAndBuildNumber: false,
	},
	install: { method: "release-testing", destination: "export" },
};

export type ExportKind = keyof typeof EXPORTS;

export const exportOptions = (teamId: string, kind: ExportKind) =>
	plist({ ...EXPORTS[kind], signingStyle: "automatic", teamID: teamId });

export const EXPORT_OPTIONS_FILE = "ios/build/ExportOptions.plist";
const EXPORT_PATH = "ios/build/export";

const productionName = (local: IosLocal) => projectName(iosConfig("production", local, true).name);

// The exported .ipa, from the app's folder. The release workflow uploads it
// to TestFlight from this path.
export const ipaPath = (local: IosLocal) => `${EXPORT_PATH}/${productionName(local)}.ipa`;

// An App Store Connect API key, which xcodebuild signs and uploads with in
// place of the account signed in to Xcode. The release workflow has no such
// account, so it sets all three variables; a Mac may set them too.
export type ApiKey = { path: string; id: string; issuer: string };

export const API_KEY_ENV = {
	path: "MAXHQ_ASC_KEY_PATH",
	id: "MAXHQ_ASC_KEY_ID",
	issuer: "MAXHQ_ASC_ISSUER_ID",
} as const;

// The key the environment names, or null when it names none. Half a key is
// an error that names what's missing and never echoes what's set.
export const apiKeyFromEnv = (env: Record<string, string | undefined>): ApiKey | null => {
	const names = Object.values(API_KEY_ENV);
	const missing = names.filter((name) => !env[name]);
	if (missing.length === names.length) return null;
	if (missing.length > 0) {
		throw new Error(`An API key needs ${names.join(", ")}; ${missing.join(", ")} isn't set.`);
	}
	return {
		path: env[API_KEY_ENV.path]!,
		id: env[API_KEY_ENV.id]!,
		issuer: env[API_KEY_ENV.issuer]!,
	};
};

const signedWith = (key: ApiKey | null): Pick<Step, "args" | "hide"> =>
	key
		? {
				args: [
					"-authenticationKeyPath",
					key.path,
					"-authenticationKeyID",
					key.id,
					"-authenticationKeyIssuerID",
					key.issuer,
				],
				hide: [key.id, key.issuer],
			}
		: { args: [] };

const prebuildStep = () =>
	expo(["prebuild", "--clean", "--platform", "ios"], expoEnv("production", true));

// The archive, from the app's folder. The release workflow carries it from
// its build job to its upload job.
export const archivePath = (local: IosLocal) => `ios/build/${productionName(local)}.xcarchive`;

// The app inside that archive.
export const archivedApp = (local: IosLocal) =>
	`${archivePath(local)}/Products/Applications/${productionName(local)}.app`;

// The app inside an exported .ipa, once unpacked.
export const appInIpa = (local: IosLocal) => `Payload/${productionName(local)}.app`;

const archiveStep = (local: IosLocal, signing: Pick<Step, "args" | "hide">): Step => {
	const name = productionName(local);
	return {
		cmd: "xcodebuild",
		args: [
			"-workspace",
			`ios/${name}.xcworkspace`,
			"-scheme",
			name,
			"-configuration",
			"Release",
			"-destination",
			"generic/platform=iOS",
			"-archivePath",
			archivePath(local),
			...signing.args,
			`DEVELOPMENT_TEAM=${local.teamId}`,
			"archive",
		],
		cwd: "app",
		...(signing.hide ? { hide: signing.hide } : {}),
	};
};

const exportStep = (local: IosLocal, key: ApiKey | null): Step => {
	const auth = signedWith(key);
	return {
		cmd: "xcodebuild",
		args: [
			"-exportArchive",
			"-archivePath",
			archivePath(local),
			"-exportOptionsPlist",
			EXPORT_OPTIONS_FILE,
			"-exportPath",
			EXPORT_PATH,
			"-allowProvisioningUpdates",
			...auth.args,
		],
		cwd: "app",
		...(auth.hide ? { hide: auth.hide } : {}),
	};
};

// Prebuild production from clean, archive it in Release and export it with
// the account signed in to Xcode, or with `key` when there is one, for
// TestFlight and an install alike. scripts/mobile.ts writes the export
// options file between prebuild and archive, since prebuild clears ios/.
export const testflightSteps = (local: IosLocal, key: ApiKey | null = null): Step[] => {
	const auth = signedWith(key);
	return [
		prebuildStep(),
		archiveStep(local, {
			args: ["-allowProvisioningUpdates", ...auth.args],
			...(auth.hide ? { hide: auth.hide } : {}),
		}),
		exportStep(local, key),
	];
};

// The release workflow archives with no key and no account, so its build
// job, which runs every package's scripts, holds nothing to sign with.
// Everything is signed ad hoc, which keeps the entitlements in the
// signature, and a job of its own signs the archive for App Store Connect
// as it exports it.
export const AD_HOC_SIGNING = [
	"CODE_SIGN_STYLE=Manual",
	"CODE_SIGN_IDENTITY=-",
	"AD_HOC_CODE_SIGNING_ALLOWED=YES",
	"PROVISIONING_PROFILE_SPECIFIER=",
];

/** The release workflow's archive: prebuild, then archive signed ad hoc. */
export const keylessArchiveSteps = (local: IosLocal): Step[] => [
	prebuildStep(),
	archiveStep(local, { args: AD_HOC_SIGNING }),
];

/** The release workflow's export of that archive, signed with the key. */
export const exportSteps = (local: IosLocal, key: ApiKey): Step[] => [exportStep(local, key)];

// What the app's signature must carry, as iosConfig asks for it: push, the
// app group, the Keychain group and time-sensitive prompts. The release
// workflow checks the ad hoc archive and the exported app, so a build that
// lost any of them fails before TestFlight. Only the exported app must say
// production for push, and Xcode fills in the Keychain group's team prefix.
export const missingEntitlements = (
	local: IosLocal,
	signed: Record<string, unknown>,
	exported: boolean
): string[] => {
	const { bundleIdentifier, appGroup } = iosConfig("production", local, true);
	const listed = (value: unknown, ok: (item: string) => boolean) =>
		Array.isArray(value) && value.some((item) => typeof item === "string" && ok(item));
	const checks: [string, (value: unknown) => boolean][] = [
		[
			"aps-environment",
			(value) => (exported ? value === "production" : typeof value === "string"),
		],
		[
			"com.apple.security.application-groups",
			(value) => listed(value, (group) => group === appGroup),
		],
		[
			"keychain-access-groups",
			(value) => listed(value, (group) => group.endsWith(bundleIdentifier)),
		],
		["com.apple.developer.usernotifications.time-sensitive", (value) => value === true],
	];
	return checks.filter(([key, ok]) => !ok(signed[key])).map(([key]) => key);
};

// The production app on the phone, next to the dev build: the release steps,
// then the app out of the exported .ipa and onto the phone.
export const installSteps = (local: IosLocal, phone: string | undefined): Step[] => [
	...testflightSteps(local),
	{ cmd: "ditto", args: ["-x", "-k", ipaPath(local), EXPORT_PATH], cwd: "app" },
	phoneInstall(phone, `${EXPORT_PATH}/${appInIpa(local)}`),
];

export type NativeBuild = {
	buildNumber: number;
	lastUpload: { buildNumber: number; fingerprint: string } | null;
};

// App Store Connect takes each build number once per version, and builds
// reach it from two places: `pnpm mobile:testflight` on a Mac, numbered by
// buildNumber in native-build.json, and the release workflow, which sets
// MAXHQ_BUILD_NUMBER. The two never share a number: checkNativeBuild keeps
// local numbers below PIPELINE_BUILD_FLOOR, and the workflow's start above
// it and only grow.
export const BUILD_NUMBER_ENV = "MAXHQ_BUILD_NUMBER";
export const PIPELINE_BUILD_FLOOR = 10_000;

// Attempts per run the numbering leaves room for. GitHub stops re-runs at
// 50 attempts.
const ATTEMPTS = 100;

// The workflow's build number for a run and its attempt: run 7, attempt 2 is
// 10702. A re-run keeps its run number, so the attempt keeps each upload
// apart, and run numbers only grow, so a later run uploads a higher number.
export const pipelineBuildNumber = (run: number, attempt: number): number => {
	if (!Number.isSafeInteger(run) || run < 1) {
		throw new Error(`The run number ${run} isn't a whole number from 1.`);
	}
	if (!Number.isSafeInteger(attempt) || attempt < 1 || attempt >= ATTEMPTS) {
		throw new Error(
			`The run attempt ${attempt} isn't a whole number from 1 to ${ATTEMPTS - 1}.`
		);
	}
	return PIPELINE_BUILD_FLOOR + run * ATTEMPTS + attempt;
};

// The build number a build gets: the workflow's when it set one, else
// native-build.json's.
export const resolveBuildNumber = (record: NativeBuild, fromEnv: string | undefined): number => {
	if (fromEnv === undefined || fromEnv === "") return record.buildNumber;
	const number = /^[1-9][0-9]*$/.test(fromEnv) ? Number(fromEnv) : Number.NaN;
	if (!(number > PIPELINE_BUILD_FLOOR)) {
		throw new Error(
			`${BUILD_NUMBER_ENV} is "${fromEnv}", but the release workflow's build numbers are whole numbers above ${PIPELINE_BUILD_FLOOR}.`
		);
	}
	return number;
};

// Why a build can't go to TestFlight, or null when it can. A native change
// needs a new build number; before an upload the number must be new anyway,
// since App Store Connect takes each one once.
export const checkNativeBuild = (
	record: NativeBuild,
	fingerprint: string,
	upload: boolean
): string | null => {
	if (record.buildNumber >= PIPELINE_BUILD_FLOOR) {
		return `Build ${record.buildNumber} is in the release workflow's range; keep buildNumber in ${NATIVE_BUILD_FILE} below ${PIPELINE_BUILD_FLOOR}.`;
	}
	const last = record.lastUpload;
	if (!last) return null;
	const raise = `raise buildNumber in ${NATIVE_BUILD_FILE}`;
	if (record.buildNumber < last.buildNumber) {
		return `Build ${record.buildNumber} is below build ${last.buildNumber}, the last one uploaded; ${raise}.`;
	}
	if (record.buildNumber > last.buildNumber) return null;
	if (fingerprint !== last.fingerprint) {
		return `The native fingerprint changed since build ${last.buildNumber} went to TestFlight; ${raise}.`;
	}
	return upload ? `Build ${last.buildNumber} is already on TestFlight; ${raise}.` : null;
};

// The dev build is rebuilt only when nothing was noted for this destination
// or its fingerprint has changed since. `device` is the iPhone and
// `simulator` the iOS Simulator; `android` is Android's dev build, which
// runs on a phone and the emulator alike.
export type DevBuilds = { device?: string; simulator?: string; android?: string };

export const needsDevBuild = (noted: DevBuilds, device: boolean | "android", fingerprint: string) =>
	noted[device === "android" ? "android" : device ? "device" : "simulator"] !== fingerprint;
