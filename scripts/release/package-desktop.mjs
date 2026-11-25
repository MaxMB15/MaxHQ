// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Builds the Windows or Linux app for one Rust target with the maxd it
// carries, and stages its installers under their release names. The release
// workflow runs it in a build job that holds no secret; sign-desktop then
// signs each installer for the updater, in a job that runs nothing built.
//
//   node scripts/release/package-desktop.mjs --target <triple> \
//     --maxd <file> --out <dir>
//
// --target is one of bundles.mjs's targets, and decides the installers: NSIS
// and MSI on Windows, AppImage, deb and rpm on Linux. --maxd is the maxd the
// app carries and runs as this computer's service: Tauri takes it from
// binaries/maxd-<triple> and puts it beside the app's program. --out gets
// the installers, named as bundles.mjs has them.
//
// The environment is package-macos.sh's: MAXHQ_RELEASES_REPO (owner/name)
// writes releases/latest/download/<manifest> into the updater's endpoints,
// MAXHQ_UPDATE_MANIFEST names that manifest (default latest.json), and
// MAXHQ_UPDATE_PUBKEY replaces tauri.conf.json's key during a key change.
// No signing key reaches the build, which runs every package's scripts.
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
import { assetName, targetInfo } from "./bundles.mjs";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const DESKTOP = join(ROOT, "apps/desktop");

// The tauri build --config that package-macos.sh writes, with the maxd
// sidecar, and the warning to print when there's one. The updater signs no
// installer here: sign-desktop does, with sign.mjs.
export const tauriConfig = (env) => {
	const config = { bundle: { createUpdaterArtifacts: false, externalBin: ["binaries/maxd"] } };
	const repo = env.MAXHQ_RELEASES_REPO;
	const pubkey = env.MAXHQ_UPDATE_PUBKEY;
	if (!repo) {
		if (pubkey) throw new Error("MAXHQ_UPDATE_PUBKEY needs MAXHQ_RELEASES_REPO");
		return {
			config,
			warning: "MAXHQ_RELEASES_REPO isn't set, so this app can't look for updates",
		};
	}
	if (!repo.includes("/")) throw new Error(`MAXHQ_RELEASES_REPO is owner/name, not ${repo}`);
	const manifest = env.MAXHQ_UPDATE_MANIFEST || "latest.json";
	if (!/^latest(-v[0-9])?\.json$/.test(manifest))
		throw new Error(
			`MAXHQ_UPDATE_MANIFEST is latest.json or latest-v<n>.json, not ${manifest}`
		);
	const updater = { endpoints: [endpoint(repo, manifest)] };
	if (pubkey) {
		if (!/^[A-Za-z0-9+/=]+$/.test(pubkey))
			throw new Error("MAXHQ_UPDATE_PUBKEY isn't a base64 public key");
		updater.pubkey = pubkey;
	}
	return { config: { ...config, plugins: { updater } } };
};

export const endpoint = (repo, manifest) =>
	`https://github.com/${repo}/releases/latest/download/${manifest}`;

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
// rpm/MaxHQ-<version>-1.x86_64.rpm. Exactly one each, or the build is wrong.
const EXTENSION = {
	nsis: "-setup.exe",
	msi: ".msi",
	appimage: ".AppImage",
	deb: ".deb",
	rpm: ".rpm",
};

export const bundleOutputs = (bundleDir, bundles) =>
	Object.fromEntries(
		bundles.map((bundle) => {
			const dir = join(bundleDir, bundle);
			const files = existsSync(dir)
				? readdirSync(dir).filter(
						(name) =>
							name.endsWith(EXTENSION[bundle]) && statSync(join(dir, name)).isFile()
					)
				: [];
			if (files.length !== 1)
				throw new Error(
					`${dir} has ${files.length === 0 ? "no" : files.length} *${EXTENSION[bundle]}, not one`
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
		},
	});
	try {
		if (!values.target || !values.maxd || !values.out)
			throw new Error("--target, --maxd and --out are required");
		const { target, os, arch, bundles } = targetInfo(values.target);
		if (!existsSync(values.maxd)) throw new Error(`there's no maxd at ${values.maxd}`);
		const { config, warning } = tauriConfig(process.env);
		if (warning) warn(warning);
		const updater = config.plugins?.updater;
		if (updater) note(`Updates come from ${updater.endpoints[0]}`);
		if (updater?.pubkey) note("The updater trusts the public key in MAXHQ_UPDATE_PUBKEY");

		const sidecar = join(DESKTOP, "src-tauri/binaries", sidecarName(target));
		mkdirSync(dirname(sidecar), { recursive: true });
		copyFileSync(values.maxd, sidecar);
		chmodSync(sidecar, 0o755);

		const targetDir = process.env.CARGO_TARGET_DIR || join(ROOT, "target");
		const release = join(targetDir, target, "release");
		const bundleDir = join(release, "bundle");
		for (const bundle of bundles)
			rmSync(join(bundleDir, bundle), { recursive: true, force: true });

		const temp = mkdtempSync(join(process.env.RUNNER_TEMP || tmpdir(), "maxhq-desktop-"));
		try {
			const configFile = join(temp, "release.json");
			writeFileSync(configFile, JSON.stringify(config));
			note(`Building MaxHQ for ${target}: ${bundles.join(", ")}`);
			const args = [
				"build",
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
		const program = join(release, os === "windows" ? "maxhq-desktop.exe" : "maxhq-desktop");
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
