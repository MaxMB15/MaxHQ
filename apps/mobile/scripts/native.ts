// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The steps behind `pnpm mobile:core`, `mobile:dev`, `mobile:testflight` and
// `mobile:install`, as plain lists of commands so the tests and --dry-run can
// read them without running anything. scripts/mobile.ts runs them.

import { iosConfig, type IosLocal } from "./ios-config.ts";

export type Step = {
	cmd: string;
	args: string[];
	env?: Record<string, string>;
	cwd?: "repo" | "app" | "core";
};

// The two Apple targets the core is built for.
export const RUST_TARGETS = ["aarch64-apple-ios", "aarch64-apple-ios-sim"] as const;

export const NATIVE_BUILD_FILE = "apps/mobile/native-build.json";
export const DEV_BUILD_FILE = "apps/mobile/.expo/maxhq-dev-build.json";

// Where a phone's dev build goes, outside ios/ so a clean prebuild keeps
// Xcode's cache.
export const DEV_DERIVED_DATA = ".expo/xcode";

// The core's React Native module, which uniffi-bindgen-react-native writes.
// It runs in the module's folder, since it reads the package.json there.
export const CORE_MODULE = "modules/maxhq-core";

// Debug for dev builds, release with thin LTO for TestFlight. Cargo builds
// each target, then ubrn writes the bindings and the xcframework from those
// builds.
export const coreSteps = (release: boolean): Step[] => [
	...RUST_TARGETS.map((target): Step => ({
		cmd: "cargo",
		args: [
			"build",
			"-p",
			"maxhq-mobile",
			"--target",
			target,
			...(release ? ["--release"] : []),
		],
		...(release ? { env: { CARGO_PROFILE_RELEASE_LTO: "thin" } } : {}),
		cwd: "repo",
	})),
	{
		cmd: "ubrn",
		args: [
			"build",
			"ios",
			"--config",
			"ubrn.config.yaml",
			"--no-cargo",
			"--and-generate",
			...(release ? ["--release"] : []),
		],
		cwd: "core",
	},
];

const expoEnv = (variant: "development" | "production", device: boolean) => ({
	APP_VARIANT: variant,
	EXPO_NO_GIT_STATUS: "1",
	...(device ? { MAXHQ_IOS_DEVICE: "1" } : {}),
});

const expo = (args: string[], env: Record<string, string>): Step => ({
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
// Connect. An install's is an .ipa signed ad hoc for the phones registered
// with the team, which keeps the production aps-environment, so the app gets
// production pushes.
const EXPORTS = {
	testflight: {
		method: "app-store-connect",
		destination: "upload",
		manageAppVersionAndBuildNumber: false,
	},
	install: { method: "release-testing", destination: "export" },
};

export type ExportKind = keyof typeof EXPORTS;

export const exportOptions = (teamId: string, kind: ExportKind) =>
	plist({ ...EXPORTS[kind], signingStyle: "automatic", teamID: teamId });

export const EXPORT_OPTIONS_FILE = "ios/build/ExportOptions.plist";
const EXPORT_PATH = "ios/build/export";

// Prebuild production from clean, archive it in Release and export it with
// the account signed in to Xcode, for TestFlight and an install alike.
// scripts/mobile.ts writes the export options file between prebuild and
// archive, since prebuild clears ios/.
const releaseSteps = (local: IosLocal) => {
	const name = projectName(iosConfig("production", local, true).name);
	const archive = `ios/build/${name}.xcarchive`;
	const steps: Step[] = [
		expo(["prebuild", "--clean", "--platform", "ios"], expoEnv("production", true)),
		{
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
				archive,
				"-allowProvisioningUpdates",
				`DEVELOPMENT_TEAM=${local.teamId}`,
				"archive",
			],
			cwd: "app",
		},
		{
			cmd: "xcodebuild",
			args: [
				"-exportArchive",
				"-archivePath",
				archive,
				"-exportOptionsPlist",
				EXPORT_OPTIONS_FILE,
				"-exportPath",
				EXPORT_PATH,
				"-allowProvisioningUpdates",
			],
			cwd: "app",
		},
	];
	return { name, steps };
};

export const testflightSteps = (local: IosLocal): Step[] => releaseSteps(local).steps;

// The production app on the phone, next to the dev build: the release steps,
// then the app out of the exported .ipa and onto the phone.
export const installSteps = (local: IosLocal, phone: string | undefined): Step[] => {
	const { name, steps } = releaseSteps(local);
	return [
		...steps,
		{ cmd: "ditto", args: ["-x", "-k", `${EXPORT_PATH}/${name}.ipa`, EXPORT_PATH], cwd: "app" },
		phoneInstall(phone, `${EXPORT_PATH}/Payload/${name}.app`),
	];
};

export type NativeBuild = {
	buildNumber: number;
	lastUpload: { buildNumber: number; fingerprint: string } | null;
};

// Why a build can't go to TestFlight, or null when it can. A native change
// needs a new build number; before an upload the number must be new anyway,
// since App Store Connect takes each one once.
export const checkNativeBuild = (
	record: NativeBuild,
	fingerprint: string,
	upload: boolean
): string | null => {
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
// or its fingerprint has changed since.
export type DevBuilds = { device?: string; simulator?: string };

export const needsDevBuild = (noted: DevBuilds, device: boolean, fingerprint: string) =>
	noted[device ? "device" : "simulator"] !== fingerprint;
