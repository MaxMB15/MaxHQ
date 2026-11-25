// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Runs the phone app's local builds. No EAS and no Expo account: signing is
// automatic with the Team ID from ios.local.json, and the upload uses the
// account signed in to Xcode, or the App Store Connect API key the
// MAXHQ_ASC_* variables name (see native.ts).
//
//   core [--release]                  the Rust core for both Apple targets
//   dev [--simulator] [--rebuild]     the core, a new dev build when the
//                                     fingerprint changed, then Metro
//   testflight                        the core in release, the check, archive
//                                     and upload
//   install                           the core in release, then the
//                                     production app archived, signed ad hoc
//                                     and installed on the paired iPhone
//   check                             the fingerprint check on its own, for CI
//   android-core [--release]          the Rust core for both Android ABIs
//   android-dev [--rebuild]           the core, a new debug APK when the
//                                     fingerprint changed, installed on the
//                                     one Android phone adb lists, then Metro
//   android-apk [--dev]               the core in release and the production
//                                     APK, signed when the ANDROID_KEY*
//                                     variables hold a key (see android.ts);
//                                     --dev builds the debug APK instead
//   android-aab                       as android-apk, as an App Bundle
//   build-number                      prints the release workflow's build
//                                     number for this run and attempt
//   archive                           the release workflow's build: the core
//                                     in release, then the production app
//                                     archived and signed ad hoc, with no key
//   export                            the release workflow's upload job: that
//                                     archive exported as an .ipa, signed
//                                     with the API key
//
// With MAXHQ_BUILD_NUMBER set, as the release workflow sets it, testflight
// numbers the build with it and leaves native-build.json alone. archive
// needs that number. archive and export each check that the app's signature
// still carries push and the app group. export runs without the packages
// installed, so it loads nothing but Node's own modules.
//
// --dry-run prints the commands instead of running them and writes nothing.

import { spawnSync } from "node:child_process";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

import {
	androidEnv,
	DEBUG_APK,
	devApkSteps,
	devInstallStep,
	newestBuildTools,
	onlyAndroidPhone,
	releaseOutput,
	releaseSteps,
	signAabSteps,
	signApkSteps,
	SIGNING_ENV,
	signingFiles,
	signingFromEnv,
	type Signing,
} from "./android.ts";
import { type IosLocal, MISSING_LOCAL, readIosLocal } from "./ios-config.ts";
import {
	ANDROID_NDK_VERSION,
	API_KEY_ENV,
	apiKeyFromEnv,
	appInIpa,
	archivedApp,
	archivePath,
	BUILD_NUMBER_ENV,
	checkNativeBuild,
	CORE_MODULE,
	coreSteps,
	DEV_BUILD_FILE,
	devBuildSteps,
	EXPORT_OPTIONS_FILE,
	exportOptions,
	exportSteps,
	installSteps,
	ipaPath,
	keylessArchiveSteps,
	metroStep,
	missingEntitlements,
	NATIVE_BUILD_FILE,
	needsDevBuild,
	onlyPhone,
	pipelineBuildNumber,
	resolveBuildNumber,
	testflightSteps,
	type DevBuilds,
	type ExportKind,
	type CorePlatform,
	type ListedDevice,
	type NativeBuild,
	type Step,
} from "./native.ts";
import { releaseExtra } from "./release-config.ts";

const appDir = resolve(import.meta.dirname, "..");
const repoDir = resolve(appDir, "../..");

const [command, ...flags] = process.argv.slice(2);
const dryRun = flags.includes("--dry-run");

const fail = (message: string): never => {
	console.error(message);
	process.exit(1);
};

// Runs the steps in order up to the first that fails, and returns that one.
const attempt = (steps: Step[]): Step | undefined =>
	steps.find((step) => {
		const env = Object.entries(step.env ?? {}).map(([key, value]) => `${key}=${value} `);
		const args = step.args.map((arg) => (step.hide?.includes(arg) ? "***" : arg));
		console.log(`$ ${env.join("")}${step.cmd} ${args.join(" ")}`);
		if (dryRun) return false;
		const result = spawnSync(step.cmd, step.args, {
			cwd: {
				repo: repoDir,
				app: appDir,
				core: join(appDir, CORE_MODULE),
				android: join(appDir, "android"),
			}[step.cwd ?? "app"],
			env: { ...process.env, ...step.env },
			stdio: "inherit",
		});
		return result.status !== 0;
	});

const run = (steps: Step[]) => {
	const failed = attempt(steps);
	if (failed) fail(`${failed.cmd} failed.`);
};

const write = (repoPath: string, text: string) => {
	if (dryRun) return console.log(`would write ${repoPath}`);
	const file = join(repoDir, repoPath);
	mkdirSync(dirname(file), { recursive: true });
	writeFileSync(file, text);
};

const readJson = <T>(repoPath: string, fallback: T): T => {
	const file = join(repoDir, repoPath);
	return existsSync(file) ? (JSON.parse(readFileSync(file, "utf8")) as T) : fallback;
};

// Loaded only when a command needs it, since export runs without packages.
// Each platform's builds are hashed on their own, so a change to Android's
// side alone never asks for a new iPhone build, nor the other way round.
const fingerprint = async (platform: CorePlatform) => {
	const { createProjectHashAsync } = await import("expo/fingerprint");
	return createProjectHashAsync(appDir, { silent: true, platforms: [platform] });
};

// devicectl installs on the one paired iPhone, named by its UDID. A dry run
// lists nothing, so its steps name a stand-in.
const pairedPhone = (): string | undefined => {
	if (dryRun) return "<phone>";
	const dir = mkdtempSync(join(tmpdir(), "maxhq-devices-"));
	try {
		const out = join(dir, "devices.json");
		const listed = spawnSync(
			"xcrun",
			["devicectl", "list", "devices", "--quiet", "--json-output", out],
			{ stdio: "ignore" }
		);
		if (listed.status !== 0 || !existsSync(out)) return undefined;
		const json = JSON.parse(readFileSync(out, "utf8")) as {
			result?: { devices?: ListedDevice[] };
		};
		return onlyPhone(json.result?.devices ?? []);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
};

const dev = async () => {
	const device = !flags.includes("--simulator");
	run(coreSteps(false));
	const hash = await fingerprint("ios");
	const noted = readJson<DevBuilds>(DEV_BUILD_FILE, {});
	if (flags.includes("--rebuild") || needsDevBuild(noted, device, hash)) {
		const steps = devBuildSteps(device, device ? pairedPhone() : undefined);
		run(steps.build);
		// Metro starts even when the phone doesn't take the build. Nothing is
		// noted then, so the next run builds and installs again.
		const failed = steps.install && attempt([steps.install]);
		if (failed) {
			console.error(
				`${failed.cmd} failed, so the phone keeps its last dev build and the next mobile:dev tries again.`
			);
		} else {
			write(
				DEV_BUILD_FILE,
				`${JSON.stringify({ ...noted, [device ? "device" : "simulator"]: hash }, null, "\t")}\n`
			);
		}
	} else {
		console.log(
			"The native side hasn't changed since the last dev build, so only Metro starts."
		);
	}
	run([metroStep()]);
};

const check = async (upload: boolean) => {
	const record =
		readJson<NativeBuild | null>(NATIVE_BUILD_FILE, null) ??
		fail(`${NATIVE_BUILD_FILE} is missing.`);
	const hash = await fingerprint("ios");
	const refusal = checkNativeBuild(record, hash, upload);
	if (refusal) fail(refusal);
	console.log(`Build ${record.buildNumber}, fingerprint ${hash}.`);
	return { record, hash };
};

// Prebuild clears ios/, so the export options go in after it.
const release = (steps: Step[], teamId: string, kind: ExportKind) => {
	const [prebuild, ...xcode] = steps;
	run([prebuild!]);
	write(`apps/mobile/${EXPORT_OPTIONS_FILE}`, exportOptions(teamId, kind));
	run(xcode);
};

const testflight = async () => {
	const local = readIosLocal(appDir) ?? fail(MISSING_LOCAL);
	const key = apiKeyFromEnv(process.env);
	if (key && !dryRun && !existsSync(key.path)) fail("The API key file isn't there.");
	// app.config.ts reads these at prebuild, after the core's long build, so a
	// bad value stops here first.
	releaseExtra(process.env);
	const numbered = process.env[BUILD_NUMBER_ENV];
	if (numbered) {
		// The workflow's numbers never meet native-build.json's, so neither
		// the check nor the record applies.
		const record = readJson<NativeBuild>(NATIVE_BUILD_FILE, {
			buildNumber: 1,
			lastUpload: null,
		});
		const buildNumber = resolveBuildNumber(record, numbered);
		run(coreSteps(true));
		console.log(`Build ${buildNumber}, numbered by ${BUILD_NUMBER_ENV}.`);
		release(testflightSteps(local, key), local.teamId, "testflight");
		if (!dryRun) console.log(`Build ${buildNumber} is on its way to TestFlight.`);
		return;
	}
	run(coreSteps(true));
	const { record, hash } = await check(true);
	release(testflightSteps(local, key), local.teamId, "testflight");
	const uploaded: NativeBuild = {
		...record,
		lastUpload: { buildNumber: record.buildNumber, fingerprint: hash },
	};
	write(NATIVE_BUILD_FILE, `${JSON.stringify(uploaded, null, "\t")}\n`);
	if (!dryRun)
		console.log(
			`Build ${record.buildNumber} is on its way to TestFlight. Commit ${NATIVE_BUILD_FILE}.`
		);
};

// The steps name the phone before anything builds, so a missing phone stops
// the install before the core.
const install = () => {
	const local = readIosLocal(appDir) ?? fail(MISSING_LOCAL);
	const steps = installSteps(local, pairedPhone());
	run(coreSteps(true));
	release(steps, local.teamId, "install");
	if (!dryRun)
		console.log(
			"MaxHQ is on the phone and works without Metro. It's a new app, so pair it with your Mac once; the next mobile:install replaces it."
		);
};

// The entitlements in an app's signature, or none when it has none.
const entitlementsOf = (app: string): Record<string, unknown> => {
	const dir = mkdtempSync(join(tmpdir(), "maxhq-entitlements-"));
	try {
		const plist = join(dir, "entitlements.plist");
		const read = spawnSync("codesign", ["-d", "--entitlements", plist, "--xml", app], {
			stdio: ["ignore", "ignore", "inherit"],
		});
		if (read.status !== 0) fail(`codesign can't read the signature of ${app}.`);
		if (!existsSync(plist)) return {};
		const json = spawnSync("plutil", ["-convert", "json", "-o", "-", plist], {
			encoding: "utf8",
		});
		if (json.status !== 0) fail(`plutil can't read the entitlements of ${app}.`);
		return JSON.parse(json.stdout) as Record<string, unknown>;
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
};

// Stops when the app's signature lost what push and the app group need.
const checkEntitlements = (local: IosLocal, app: string, exported: boolean) => {
	if (dryRun) return console.log(`would check the entitlements of ${app}`);
	const missing = missingEntitlements(local, entitlementsOf(app), exported);
	if (missing.length > 0) {
		fail(
			`The signature of ${app} is missing ${missing.join(", ")}, so push or the app group wouldn't work.`
		);
	}
	console.log(`The signature of ${app} carries push and the app group.`);
};

// The release workflow's build job. No key and no account reach it, since
// it runs every package's scripts; the archive is signed ad hoc.
const archive = () => {
	const numbered = process.env[BUILD_NUMBER_ENV];
	if (!numbered) fail(`archive is for the release workflow, which sets ${BUILD_NUMBER_ENV}.`);
	if (apiKeyFromEnv(process.env)) {
		fail(
			`archive signs ad hoc and takes no App Store Connect key; leave ${Object.values(API_KEY_ENV).join(", ")} unset.`
		);
	}
	const local = readIosLocal(appDir) ?? fail(MISSING_LOCAL);
	releaseExtra(process.env);
	const buildNumber = resolveBuildNumber({ buildNumber: 1, lastUpload: null }, numbered);
	run(coreSteps(true));
	console.log(`Build ${buildNumber}, numbered by ${BUILD_NUMBER_ENV}.`);
	run(keylessArchiveSteps(local));
	checkEntitlements(local, join(appDir, archivedApp(local)), false);
	if (!dryRun)
		console.log(`Build ${buildNumber} is archived in apps/mobile/${archivePath(local)}.`);
};

// The release workflow's upload job: the archive from the build job,
// exported and signed for App Store Connect with the key. It needs no
// packages, so the job installs none while the key is on disk.
const exportArchive = () => {
	const local = readIosLocal(appDir) ?? fail(MISSING_LOCAL);
	const key =
		apiKeyFromEnv(process.env) ??
		fail(
			`export signs with an App Store Connect key, which ${Object.values(API_KEY_ENV).join(", ")} name.`
		);
	if (!dryRun && !existsSync(key.path)) fail("The API key file isn't there.");
	if (!dryRun && !existsSync(join(appDir, archivePath(local))))
		fail(`There's no archive at apps/mobile/${archivePath(local)}.`);
	write(`apps/mobile/${EXPORT_OPTIONS_FILE}`, exportOptions(local.teamId, "ipa"));
	run(exportSteps(local, key));
	if (dryRun) return checkEntitlements(local, appInIpa(local), true);
	const dir = mkdtempSync(join(tmpdir(), "maxhq-ipa-"));
	try {
		run([{ cmd: "ditto", args: ["-x", "-k", join(appDir, ipaPath(local)), dir] }]);
		checkEntitlements(local, join(dir, appInIpa(local)), true);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
	console.log(`The .ipa is signed for TestFlight in apps/mobile/${ipaPath(local)}.`);
};

// The Android builds read the SDK from ANDROID_HOME and the pinned NDK from
// ANDROID_NDK_HOME, which is filled in from the SDK when it's unset. A
// missing NDK stops here, before the core's long build, with the command
// that installs it.
const setUpAndroid = () => {
	const { sdk, add } = androidEnv(process.env);
	Object.assign(process.env, add);
	const ndk = process.env.ANDROID_NDK_HOME!;
	if (!dryRun && !existsSync(ndk)) {
		fail(`There's no NDK at ${ndk}. Install it with: sdkmanager "ndk;${ANDROID_NDK_VERSION}"`);
	}
	return sdk;
};

// The upload key leaves the environment as soon as it's read, so Gradle,
// prebuild and the plugins they run never see it.
const takeSigning = (): Signing | null => {
	const signing = signingFromEnv(process.env);
	for (const name of Object.values(SIGNING_ENV)) delete process.env[name];
	return signing;
};

// The one Android phone adb lists, or none. A dry run lists nothing, so its
// steps name a stand-in.
const androidPhone = (): string | undefined => {
	if (dryRun) return "<phone>";
	const listed = spawnSync("adb", ["devices", "-l"], { encoding: "utf8" });
	return listed.status === 0 ? onlyAndroidPhone(listed.stdout) : undefined;
};

const androidDev = async () => {
	setUpAndroid();
	run(coreSteps(false, "android"));
	const hash = await fingerprint("android");
	const noted = readJson<DevBuilds>(DEV_BUILD_FILE, {});
	if (flags.includes("--rebuild") || needsDevBuild(noted, "android", hash)) {
		run(devApkSteps());
		// Metro starts even when no phone takes the build. Nothing is noted
		// then, so the next run builds and installs again.
		const phone = androidPhone();
		const failed = phone ? attempt([devInstallStep(phone)]) : undefined;
		if (!phone || failed) {
			console.error(
				`${phone ? "adb install failed" : "Found no Android phone, or more than one"}, so the debug APK stays in apps/mobile/${DEBUG_APK} and the next mobile:android:dev tries again.`
			);
		} else {
			write(DEV_BUILD_FILE, `${JSON.stringify({ ...noted, android: hash }, null, "\t")}\n`);
		}
	} else {
		console.log(
			"The native side hasn't changed since the last dev build, so only Metro starts."
		);
	}
	run([metroStep()]);
};

// Writes the key into files only this user can read, runs the signing steps
// with them and removes them again, whether or not the steps worked.
const signWith = (signing: Signing, steps: (dir: string) => Step[]) => {
	if (dryRun) return run(steps("<keys>"));
	const dir = mkdtempSync(join(tmpdir(), "maxhq-signing-"));
	try {
		const files = signingFiles(dir);
		writeFileSync(files.keystore, signing.keystore, { mode: 0o600 });
		writeFileSync(files.storePassword, signing.storePassword, { mode: 0o600 });
		writeFileSync(files.keyPassword, signing.keyPassword, { mode: 0o600 });
		run(steps(dir));
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
};

// The newest build-tools the SDK has, for zipalign and apksigner.
const buildTools = (sdk: string) => {
	const folder = join(sdk, "build-tools");
	const version = existsSync(folder) ? newestBuildTools(readdirSync(folder)) : undefined;
	if (version) return join(folder, version);
	if (dryRun) return join(folder, "<version>");
	return fail(`The SDK at ${sdk} has no build-tools. Install them with sdkmanager.`);
};

const jarsigner = () =>
	process.env.JAVA_HOME ? join(process.env.JAVA_HOME, "bin", "jarsigner") : "jarsigner";

const androidRelease = (kind: "apk" | "aab") => {
	const signing = takeSigning();
	const sdk = setUpAndroid();
	if (kind === "apk" && flags.includes("--dev")) {
		run(coreSteps(false, "android"));
		run(devApkSteps());
		return console.log(`The debug APK is in apps/mobile/${DEBUG_APK}.`);
	}
	run(coreSteps(true, "android"));
	run(releaseSteps(kind));
	if (signing) {
		const hide = (steps: Step[]) => steps.map((step) => ({ ...step, hide: [signing.alias] }));
		signWith(signing, (dir) =>
			hide(
				kind === "apk"
					? signApkSteps(buildTools(sdk), signingFiles(dir), signing.alias)
					: signAabSteps(jarsigner(), signingFiles(dir), signing.alias)
			)
		);
	}
	const signed = signing ? "signed with the upload key" : "unsigned";
	console.log(
		`The ${kind === "apk" ? "APK" : "App Bundle"}, ${signed}, is in apps/mobile/${releaseOutput(kind, signing !== null)}.`
	);
};

const main = async () => {
	switch (command) {
		case "core":
			return run(coreSteps(flags.includes("--release")));
		case "dev":
			return dev();
		case "testflight":
			return testflight();
		case "install":
			return install();
		case "check":
			return void (await check(false));
		case "android-core":
			setUpAndroid();
			return run(coreSteps(flags.includes("--release"), "android"));
		case "android-dev":
			return androidDev();
		case "android-apk":
			return androidRelease("apk");
		case "android-aab":
			return androidRelease("aab");
		case "archive":
			return archive();
		case "export":
			return exportArchive();
		case "build-number":
			return console.log(
				pipelineBuildNumber(
					Number(process.env.GITHUB_RUN_NUMBER),
					Number(process.env.GITHUB_RUN_ATTEMPT)
				)
			);
		default:
			fail(
				"Usage: mobile.ts core|dev|testflight|install|check|build-number|archive|export|android-core|android-dev|android-apk|android-aab [--dry-run]"
			);
	}
};

main().catch((error: unknown) => fail(error instanceof Error ? error.message : String(error)));
