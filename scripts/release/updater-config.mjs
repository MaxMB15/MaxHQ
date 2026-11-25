// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The `tauri build --config` both desktop builds pass, package-macos.sh for
// the Mac and package-desktop.mjs for Windows and Linux: no updater
// artifacts, since sign.mjs signs them, and the updater's endpoint and key
// from the environment.
//
//   node scripts/release/updater-config.mjs
//
// prints the config for package-macos.sh on stdout, and on stderr where
// updates come from, or a warning when the app can't look for any.
//
// MAXHQ_RELEASES_REPO (owner/name) writes releases/latest/download/<manifest>
// into the updater's endpoints, MAXHQ_UPDATE_MANIFEST names that manifest
// (default latest.json), and MAXHQ_UPDATE_PUBKEY replaces tauri.conf.json's
// key during a key change. The repository goes in at build time, so no
// tracked file names its owner.

export const endpoint = (repo, manifest) =>
	`https://github.com/${repo}/releases/latest/download/${manifest}`;

// The config, with `bundle` added to its bundle section, what it says about
// updates as notes, and the warning to print when the app gets no updater.
export const tauriConfig = (env, bundle = {}) => {
	const config = { bundle: { createUpdaterArtifacts: false, ...bundle } };
	const repo = env.MAXHQ_RELEASES_REPO;
	const pubkey = env.MAXHQ_UPDATE_PUBKEY;
	if (!repo) {
		if (pubkey) throw new Error("MAXHQ_UPDATE_PUBKEY needs MAXHQ_RELEASES_REPO");
		return {
			config,
			notes: [],
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
	const notes = [`Updates come from ${updater.endpoints[0]}`];
	if (pubkey) {
		if (!/^[A-Za-z0-9+/=]+$/.test(pubkey))
			throw new Error("MAXHQ_UPDATE_PUBKEY isn't a base64 public key");
		updater.pubkey = pubkey;
		notes.push("The updater trusts the public key in MAXHQ_UPDATE_PUBKEY");
	}
	return { config: { ...config, plugins: { updater } }, notes };
};

const main = () => {
	const ci = Boolean(process.env.GITHUB_ACTIONS);
	try {
		const { config, notes, warning } = tauriConfig(process.env);
		for (const note of notes) console.error(`==> ${note}`);
		if (warning) console.error(ci ? `::warning::${warning}` : `warning: ${warning}`);
		console.log(JSON.stringify(config));
	} catch (error) {
		console.error(`${ci ? "::error::" : ""}updater-config.mjs: ${error.message}`);
		process.exit(1);
	}
};

if (import.meta.main) main();
