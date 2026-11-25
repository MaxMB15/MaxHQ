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
//   android-release                   the release workflow's Android build:
//                                     the core in release, then the APK and
//                                     the App Bundle, unsigned, with no key
//   android-sign <build> <out>        the release workflow's signing job:
//                                     those two, from the build job's folder,
//                                     signed with the ANDROID_KEY* variables'
//                                     key into <out> as MaxHQ.apk and
//                                     MaxHQ.aab; with no key it says so and
//                                     writes nothing
//   build-number                      prints the release workflow's build
//                                     number for this run and attempt
//   archive                           the release workflow's build: the core
//                                     in release, then the production app
//                                     archived and signed ad hoc, with no key
//   export                            the release workflow's upload job: that
//                                     archive exported as an .ipa, signed
//                                     with the API key
//   write-ids                         the release workflows' ios.local.json,
//                                     from IOS_TEAM_ID and IOS_BUNDLE_ID
//
// With MAXHQ_BUILD_NUMBER set, as the release workflow sets it, testflight
// numbers the build with it and leaves native-build.json alone. archive and
// android-release need that number, which is also the Android version code.
// archive and export each check that the app's signature still carries push
// and the app group. export, android-sign and write-ids run without the
// packages installed, so they load nothing but Node's own modules.
//
// --dry-run prints the commands instead of running them and writes nothing.

import { spawnSync } from "node:child_process";
import {
	appendFileSync,
	copyFileSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

import {
	androidEnv,
	DEBUG_APK,
	devApkSteps,
	devInstallStep,
	newestBuildTools,
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
	type Signing,
	type SigningFiles,
	UNSIGNED_APK,
} from "./android.ts";
import {
	type IosLocal,
	iosLocalFromEnv,
	LOCAL_FILE,
	MISSING_LOCAL,
	readIosLocal,
} from "./ios-config.ts";
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
// The arguments that aren't flags, as paths from where it was started.
const paths = flags.filter((flag) => !flag.startsWith("--")).map((path) => resolve(path));

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

// The dev loop both phones share: the core, a new dev build when the
// fingerprint changed or --rebuild asks, then Metro. `build` builds and
// installs, and returns why the phone didn't take the build, if it didn't.
// Metro starts even then. Nothing is noted, so the next run builds and
// installs again.
const devLoop = async (
	platform: CorePlatform,
	device: boolean | "android",
	build: () => string | undefined
) => {
	run(coreSteps(false, platform));
	const hash = await fingerprint(platform);
	const noted = readJson<DevBuilds>(DEV_BUILD_FILE, {});
	if (flags.includes("--rebuild") || needsDevBuild(noted, device, hash)) {
		const missed = build();
		if (missed) {
			console.error(missed);
		} else {
			const key = device === "android" ? "android" : device ? "device" : "simulator";
			write(DEV_BUILD_FILE, `${JSON.stringify({ ...noted, [key]: hash }, null, "\t")}\n`);
		}
	} else {
		console.log(
			"The native side hasn't changed since the last dev build, so only Metro starts."
		);
	}
	run([metroStep()]);
};

const dev = () => {
	const device = !flags.includes("--simulator");
	return devLoop("ios", device, () => {
		const steps = devBuildSteps(device, device ? pairedPhone() : undefined);
		run(steps.build);
		const failed = steps.install && attempt([steps.install]);
		return failed
			? `${failed.cmd} failed, so the phone keeps its last dev build and the next mobile:dev tries again.`
			: undefined;
	});
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

const androidDev = () => {
	setUpAndroid();
	return devLoop("android", "android", () => {
		run(devApkSteps());
		const phone = androidPhone();
		const failed = phone ? attempt([devInstallStep(phone)]) : undefined;
		return !phone || failed
			? `${phone ? "adb install failed" : "Found no Android phone, or more than one"}, so the debug APK stays in apps/mobile/${DEBUG_APK} and the next mobile:android:dev tries again.`
			: undefined;
	});
};

// Writes the key into files only this user can read, runs the signing steps
// with them and removes them again, whether or not the steps worked. The
// printed commands hide the key's alias.
const signWith = (signing: Signing, steps: (files: SigningFiles) => Step[]) => {
	const hidden = (files: SigningFiles) =>
		steps(files).map((step) => ({ ...step, hide: [signing.alias] }));
	if (dryRun) return run(hidden(signingFiles("<keys>")));
	const dir = mkdtempSync(join(tmpdir(), "maxhq-signing-"));
	try {
		const files = signingFiles(dir);
		writeFileSync(files.keystore, signing.keystore, { mode: 0o600 });
		writeFileSync(files.storePassword, signing.storePassword, { mode: 0o600 });
		writeFileSync(files.keyPassword, signing.keyPassword, { mode: 0o600 });
		run(hidden(files));
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
		signWith(signing, (files) =>
			kind === "apk"
				? signApkSteps(buildTools(sdk), files, signing.alias)
				: signAabSteps(jarsigner(), files, signing.alias)
		);
	}
	const signed = signing ? "signed with the upload key" : "unsigned";
	console.log(
		`The ${kind === "apk" ? "APK" : "App Bundle"}, ${signed}, is in apps/mobile/${releaseOutput(kind, signing !== null)}.`
	);
};

// The release workflow's Android build job. No key reaches it, since it
// runs every package's scripts: the APK and the App Bundle come out
// unsigned, and android-sign signs them in a job that runs nothing built.
const androidReleaseBuild = () => {
	const numbered = process.env[BUILD_NUMBER_ENV];
	if (!numbered)
		fail(`android-release is for the release workflow, which sets ${BUILD_NUMBER_ENV}.`);
	const names = Object.values(SIGNING_ENV);
	if (names.some((name) => process.env[name]))
		fail(
			`android-release builds unsigned and takes no upload key; leave ${names.join(", ")} unset.`
		);
	// The package is the iPhone's bundle ID, so a release never goes out as
	// the placeholder app.config.ts falls back to.
	if (!readIosLocal(appDir)) fail(MISSING_LOCAL);
	releaseExtra(process.env);
	const buildNumber = resolveBuildNumber({ buildNumber: 1, lastUpload: null }, numbered);
	setUpAndroid();
	run(coreSteps(true, "android"));
	console.log(`Build ${buildNumber}, numbered by ${BUILD_NUMBER_ENV}.`);
	run(releaseSteps("both"));
	if (!dryRun)
		console.log(
			`Build ${buildNumber} is unsigned in apps/mobile/${UNSIGNED_APK} and apps/mobile/${RELEASE_AAB}.`
		);
};

// The release workflow's Android signing job: the build job's APK and App
// Bundle, copied from <build> back to where Gradle wrote them, signed with
// the upload key, and copied to <out> as MaxHQ.apk and MaxHQ.aab. With none
// of the ANDROID_KEY* variables set it says so and writes nothing, so the
// release carries no Android build until the owner adds the key; half a key
// fails before anything is written. It needs the SDK's build-tools and the
// JDK, and no packages or NDK.
const androidSign = () => {
	const [build, out] =
		paths.length === 2
			? (paths as [string, string])
			: fail("Usage: mobile.ts android-sign <build> <out>");
	const unsigned = {
		apk: join(build, basename(UNSIGNED_APK)),
		aab: join(build, basename(RELEASE_AAB)),
	};
	for (const file of Object.values(unsigned))
		if (!dryRun && !existsSync(file)) fail(`${build} has no ${basename(file)}.`);
	const signing = takeSigning();
	if (!signing) {
		const message =
			"There's no Android upload key, so the APK and the App Bundle stay unsigned and this release carries no Android build. .github/RELEASING.md says where the key goes.";
		if (!process.env.GITHUB_ACTIONS) return console.log(`warning: ${message}`);
		console.log(`::warning::${message}`);
		const summary = process.env.GITHUB_STEP_SUMMARY;
		if (summary) appendFileSync(summary, `### Android build unsigned\n${message}\n`);
		return;
	}
	const { sdk } = androidEnv(process.env);
	const copy = (from: string, to: string) => {
		if (dryRun) return console.log(`would copy ${from} to ${to}`);
		mkdirSync(dirname(to), { recursive: true });
		copyFileSync(from, to);
	};
	if (!dryRun) rmSync(join(appDir, RELEASE_APK), { force: true });
	copy(unsigned.apk, join(appDir, UNSIGNED_APK));
	copy(unsigned.aab, join(appDir, RELEASE_AAB));
	signWith(signing, (files) => [
		...signApkSteps(buildTools(sdk), files, signing.alias),
		...signAabSteps(jarsigner(), files, signing.alias),
	]);
	const signed = { apk: join(out, "MaxHQ.apk"), aab: join(out, "MaxHQ.aab") };
	copy(join(appDir, RELEASE_APK), signed.apk);
	copy(join(appDir, RELEASE_AAB), signed.aab);
	if (!dryRun)
		for (const file of Object.values(signed))
			console.log(`Signed with the upload key: ${file}, ${statSync(file).size} bytes.`);
};

// The release workflows' ios.local.json, from the IOS_TEAM_ID and
// IOS_BUNDLE_ID variables, checked as the builds read it.
const writeIds = () => write(LOCAL_FILE, iosLocalFromEnv(process.env));

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
		case "android-release":
			return androidReleaseBuild();
		case "android-sign":
			return androidSign();
		case "write-ids":
			return writeIds();
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
				"Usage: mobile.ts core|dev|testflight|install|check|build-number|archive|export|write-ids|android-core|android-dev|android-apk|android-aab|android-release|android-sign [--dry-run]"
			);
	}
};

main().catch((error: unknown) => fail(error instanceof Error ? error.message : String(error)));
