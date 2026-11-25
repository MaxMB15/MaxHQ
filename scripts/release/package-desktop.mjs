// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Builds the Windows or Linux app for one Rust target with the maxd it
// carries, and stages its installers under their release names. The release
// workflow runs it in a build job that holds no secret; sign-desktop then
// signs each installer for the updater, in a job that runs nothing built.
//
//   node scripts/release/package-desktop.mjs [--debug] --target <triple> \
//     --maxd <file> --out <dir>
//
// --target is one of bundles.mjs's targets, and decides the installers: NSIS
// and MSI on Windows, AppImage, deb and rpm on Linux. --maxd is the maxd the
// app carries and runs as this computer's service: Tauri takes it from
// binaries/maxd-<triple> and puts it beside the app's program. --out gets
// the installers, named as bundles.mjs has them. --debug makes the unsigned
// debug build CI's desktop-linux and desktop-windows jobs pack and check.
//
// The environment is package-macos.sh's, read by updater-config.mjs: the
// updater's endpoint and key. No signing key reaches the build, which runs
// every package's scripts.
//
// Phase 1: the installers carry no fleet manifest or Linux maxd for other
// devices, so these apps can't add or update other devices with a signed
// maxd yet. .github/RELEASING.md has phase 2.

import { spawnSync } from "node:child_process";
import {
	chmodSync,
	copyFileSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readdirSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { SUFFIX, assetName, targetInfo } from "./bundles.mjs";
import { tauriConfig } from "./updater-config.mjs";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const DESKTOP = join(ROOT, "apps/desktop");

// The name Tauri looks for the sidecar under, in src-tauri/binaries.
export const sidecarName = (target) =>
	target.includes("-windows-") ? `maxd-${target}.exe` : `maxd-${target}`;

// The environment the build runs in: this one without the update keys or
// Apple's secrets, which Tauri would sign with.
export const buildEnv = (env, os) => {
	const clean = Object.fromEntries(
		Object.entries(env).filter(
			([name]) => !name.startsWith("TAURI_SIGNING_PRIVATE_KEY") && !name.startsWith("APPLE_")
		)
	);
	// linuxdeploy runs without mounting itself through FUSE.
	if (os === "linux") clean.APPIMAGE_EXTRACT_AND_RUN = "1";
	return clean;
};

// Each bundle type's file in its folder under the bundle folder, as Tauri
// names it: nsis/MaxHQ_<version>_x64-setup.exe, msi/MaxHQ_<version>_x64_en-US.msi,
// appimage/MaxHQ_<version>_amd64.AppImage, deb/MaxHQ_<version>_amd64.deb and
// rpm/MaxHQ-<version>-1.x86_64.rpm, each ending as its release asset does.
// Exactly one each, or the build is wrong.
export const bundleOutputs = (bundleDir, bundles) =>
	Object.fromEntries(
		bundles.map((bundle) => {
			const dir = join(bundleDir, bundle);
			const files = existsSync(dir)
				? readdirSync(dir).filter(
						(name) =>
							name.endsWith(SUFFIX[bundle]) && statSync(join(dir, name)).isFile()
					)
				: [];
			if (files.length !== 1)
				throw new Error(
					`${dir} has ${files.length === 0 ? "no" : files.length} *${SUFFIX[bundle]}, not one`
				);
			return [bundle, join(dir, files[0])];
		})
	);

// Whether a file's bytes hold the text, as smoke-test.sh's grep -aqF checks.
export const holds = (file, text) => readFileSync(file).includes(Buffer.from(text));

// The Tauri CLI's script, run with this Node, so no .cmd shim needs a shell
// on Windows.
const tauriCli = () => {
	const require = createRequire(join(DESKTOP, "package.json"));
	const manifest = require.resolve("@tauri-apps/cli/package.json");
	return join(dirname(manifest), JSON.parse(readFileSync(manifest, "utf8")).bin.tauri);
};

const note = (text) => console.log(`==> ${text}`);
const warn = (text) =>
	console.log(process.env.GITHUB_ACTIONS ? `::warning::${text}` : `warning: ${text}`);

const main = () => {
	const { values } = parseArgs({
		options: {
			target: { type: "string" },
			maxd: { type: "string" },
			out: { type: "string" },
			debug: { type: "boolean", default: false },
		},
	});
	try {
		if (!values.target || !values.maxd || !values.out)
			throw new Error("--target, --maxd and --out are required");
		const { target, os, arch, bundles } = targetInfo(values.target);
		if (!existsSync(values.maxd)) throw new Error(`there's no maxd at ${values.maxd}`);
		const { config, notes, warning } = tauriConfig(process.env, {
			externalBin: ["binaries/maxd"],
		});
		for (const text of notes) note(text);
		// A debug build is for CI's checks, which ship nothing to update.
		if (warning) (values.debug ? note : warn)(warning);
		const updater = config.plugins?.updater;

		const sidecar = join(DESKTOP, "src-tauri/binaries", sidecarName(target));
		mkdirSync(dirname(sidecar), { recursive: true });
		copyFileSync(values.maxd, sidecar);
		chmodSync(sidecar, 0o755);

		const targetDir = process.env.CARGO_TARGET_DIR || join(ROOT, "target");
		const profile = join(targetDir, target, values.debug ? "debug" : "release");
		const bundleDir = join(profile, "bundle");
		for (const bundle of bundles)
			rmSync(join(bundleDir, bundle), { recursive: true, force: true });

		const temp = mkdtempSync(join(process.env.RUNNER_TEMP || tmpdir(), "maxhq-desktop-"));
		try {
			const configFile = join(temp, "release.json");
			writeFileSync(configFile, JSON.stringify(config));
			note(`Building MaxHQ for ${target}: ${bundles.join(", ")}`);
			const args = [
				"build",
				...(values.debug ? ["--debug"] : []),
				"--target",
				target,
				"--bundles",
				bundles.join(","),
				"--config",
				configFile,
			];
			const run = spawnSync(process.execPath, [tauriCli(), ...args], {
				cwd: DESKTOP,
				env: buildEnv(process.env, os),
				stdio: "inherit",
			});
			if (run.status !== 0)
				throw new Error(
					`tauri build failed (${run.error?.message ?? `exit ${run.status}`})`
				);
		} finally {
			rmSync(temp, { recursive: true, force: true });
		}

		// The program holds the updater's endpoint and key, as the Mac's does.
		const program = join(profile, os === "windows" ? "maxhq-desktop.exe" : "maxhq-desktop");
		if (updater && !holds(program, updater.endpoints[0]))
			throw new Error(`${program} has no update endpoint ${updater.endpoints[0]}`);
		if (updater?.pubkey && !holds(program, updater.pubkey))
			throw new Error(`${program} doesn't hold MAXHQ_UPDATE_PUBKEY`);

		mkdirSync(values.out, { recursive: true });
		for (const [bundle, file] of Object.entries(bundleOutputs(bundleDir, bundles))) {
			const asset = join(values.out, assetName(arch, bundle));
			copyFileSync(file, asset);
			note(`${assetName(arch, bundle)} from ${file} (${statSync(asset).size} bytes)`);
		}
	} catch (error) {
		console.error(
			process.env.GITHUB_ACTIONS
				? `::error::package-desktop.mjs: ${error.message}`
				: `package-desktop.mjs: ${error.message}`
		);
		process.exit(1);
	}
};

if (import.meta.main) main();
