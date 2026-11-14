// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Writes latest.json, the static manifest the app's updater fetches from the
// newest release: the version, its notes, and for each Mac the updater
// archive's URL and the contents of its .sig.
//
//   node scripts/release/latest-json.mjs --version 0.2.0 --repo <owner>/<repo> \
//     --sig aarch64=<file.sig> --sig x64=<file.sig> [--notes-file <file>] --out latest.json
//
// The archives are named MaxHQ_<version>_<arch>.app.tar.gz, arch being
// aarch64 or x64 as the release assets have it.

import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

// The release asset's arch, and the platform key the updater looks up.
export const PLATFORMS = { aarch64: "darwin-aarch64", x64: "darwin-x86_64" };

const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

export const archiveName = (version, arch) => `MaxHQ_${version}_${arch}.app.tar.gz`;

export const latestJson = ({ version, repo, notes = "", signatures, now = new Date() }) => {
	if (!SEMVER.test(version)) throw new Error(`the version "${version}" isn't X.Y.Z`);
	if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error(`"${repo}" isn't <owner>/<repo>`);
	const platforms = {};
	for (const [arch, signature] of Object.entries(signatures)) {
		const platform = PLATFORMS[arch];
		if (!platform) throw new Error(`the arch is aarch64 or x64, not "${arch}"`);
		if (!signature.trim()) throw new Error(`the ${arch} signature is empty`);
		platforms[platform] = {
			signature: signature.trim(),
			url: `https://github.com/${repo}/releases/download/v${version}/${archiveName(version, arch)}`,
		};
	}
	if (Object.keys(platforms).length === 0) throw new Error("no signatures given");
	return `${JSON.stringify(
		{
			version,
			notes: notes.trim(),
			pub_date: now.toISOString().replace(/\.\d{3}Z$/, "Z"),
			platforms,
		},
		null,
		2
	)}\n`;
};

const main = () => {
	const { values } = parseArgs({
		options: {
			version: { type: "string" },
			repo: { type: "string" },
			sig: { type: "string", multiple: true, default: [] },
			"notes-file": { type: "string" },
			out: { type: "string" },
		},
	});
	try {
		if (!values.version || !values.repo || !values.out)
			throw new Error("--version, --repo and --out are required");
		const signatures = {};
		for (const pair of values.sig) {
			const at = pair.indexOf("=");
			if (at < 1) throw new Error(`"${pair}" isn't arch=file`);
			signatures[pair.slice(0, at)] = readFileSync(pair.slice(at + 1), "utf8");
		}
		const notes = values["notes-file"] ? readFileSync(values["notes-file"], "utf8") : "";
		const text = latestJson({ version: values.version, repo: values.repo, notes, signatures });
		writeFileSync(values.out, text);
		console.log(`latest.json for ${values.version}: ${Object.keys(signatures).join(", ")}`);
	} catch (error) {
		console.error(`latest-json.mjs: ${error.message}`);
		process.exit(1);
	}
};

if (import.meta.main) main();
