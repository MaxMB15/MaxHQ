// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Writes latest.json, the static manifest the app's updater fetches from the
// newest release: the version, its notes, and for each Mac the updater
// archive's URL and the contents of its .sig.
//
//   node scripts/release/latest-json.mjs --version 0.2.0 --repo <owner>/<repo> \
//     --sig aarch64=<file.sig> --sig x64=<file.sig> [--notes-file <file>] \
//     [--key <public key>] --out latest.json
//
// The archives are named MaxHQ_<version>_<arch>.app.tar.gz, arch being
// aarch64 or x64 as the release assets have it. With --key (base64, as
// tauri.conf.json holds it), every signature must be made with that key, so a
// key change can't put a signature into the manifest for the other key.

import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { keyIdText, readPublicKey } from "./sign.mjs";

// The release asset's arch, and the platform key the updater looks up.
export const PLATFORMS = { aarch64: "darwin-aarch64", x64: "darwin-x86_64" };

const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

export const archiveName = (version, arch) => `MaxHQ_${version}_${arch}.app.tar.gz`;

// The ID of the key a .sig was made with: the 8 bytes after "ED" in its
// signature line.
export const signatureKeyId = (signature) => {
	const lines = Buffer.from(signature.trim(), "base64").toString("utf8").split("\n");
	const box = Buffer.from(lines[1]?.trim() ?? "", "base64");
	if (box.length !== 74 || box.subarray(0, 2).toString("latin1") !== "ED")
		throw new Error("that isn't a minisign signature");
	return box.subarray(2, 10);
};

export const latestJson = ({ version, repo, notes = "", signatures, key, now = new Date() }) => {
	if (!SEMVER.test(version)) throw new Error(`the version "${version}" isn't X.Y.Z`);
	if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error(`"${repo}" isn't <owner>/<repo>`);
	const keyId = key === undefined ? null : readPublicKey(key).keyId;
	const platforms = {};
	for (const [arch, signature] of Object.entries(signatures)) {
		const platform = PLATFORMS[arch];
		if (!platform) throw new Error(`the arch is aarch64 or x64, not "${arch}"`);
		if (!signature.trim()) throw new Error(`the ${arch} signature is empty`);
		if (keyId) {
			const signer = signatureKeyId(signature);
			if (!signer.equals(keyId))
				throw new Error(
					`the ${arch} signature is by key ${keyIdText(signer)}, not ${keyIdText(keyId)}`
				);
		}
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
			key: { type: "string" },
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
		const { version, repo, key } = values;
		const text = latestJson({ version, repo, notes, signatures, key });
		writeFileSync(values.out, text);
		console.log(`${values.out} for ${version}: ${Object.keys(signatures).join(", ")}`);
	} catch (error) {
		console.error(`latest-json.mjs: ${error.message}`);
		process.exit(1);
	}
};

if (import.meta.main) main();
