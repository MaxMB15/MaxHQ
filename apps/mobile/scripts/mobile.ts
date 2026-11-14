// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Runs the phone app's local builds. No EAS and no Expo account: signing is
// automatic with the Team ID from ios.local.json, and the upload uses the
// account signed in to Xcode.
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
//
// --dry-run prints the commands instead of running them and writes nothing.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

import { createProjectHashAsync } from "expo/fingerprint";

import { MISSING_LOCAL, readIosLocal } from "./ios-config.ts";
import {
	checkNativeBuild,
	CORE_MODULE,
	coreSteps,
	DEV_BUILD_FILE,
	devBuildSteps,
	EXPORT_OPTIONS_FILE,
	exportOptions,
	installSteps,
	metroStep,
	NATIVE_BUILD_FILE,
	needsDevBuild,
	onlyPhone,
	testflightSteps,
	type DevBuilds,
	type ExportKind,
	type ListedDevice,
	type NativeBuild,
	type Step,
} from "./native.ts";

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
		console.log(`$ ${env.join("")}${step.cmd} ${step.args.join(" ")}`);
		if (dryRun) return false;
		const result = spawnSync(step.cmd, step.args, {
			cwd: { repo: repoDir, app: appDir, core: join(appDir, CORE_MODULE) }[step.cwd ?? "app"],
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

const fingerprint = () => createProjectHashAsync(appDir, { silent: true });

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
	const hash = await fingerprint();
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
	const hash = await fingerprint();
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
	run(coreSteps(true));
	const { record, hash } = await check(true);
	release(testflightSteps(local), local.teamId, "testflight");
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
		default:
			fail("Usage: mobile.ts core|dev|testflight|install|check [--dry-run]");
	}
};

main().catch((error: unknown) => fail(error instanceof Error ? error.message : String(error)));
