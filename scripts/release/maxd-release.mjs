// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Writes maxd-release.json, the fleet manifest a release carries in the app's
// Resources: its version and the sha256 of each maxd it ships, keyed by
// `maxd-<rust target>`. sign.mjs then signs it, and maxd checks an uploaded
// binary against it before it replaces itself.
//
//   node scripts/release/maxd-release.mjs --version 0.2.0 --out <file> \
//     maxd-aarch64-apple-darwin=<path> maxd-x86_64-unknown-linux-musl=<path> …
//
// Hash the Mac's maxd after codesign, since signing changes its bytes. The
// Windows maxd's key has no .exe, as maxd looks it up by its target alone.

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

export const TARGETS = [
	"aarch64-apple-darwin",
	"x86_64-apple-darwin",
	"aarch64-unknown-linux-musl",
	"x86_64-unknown-linux-musl",
	"aarch64-pc-windows-msvc",
	"x86_64-pc-windows-msvc",
];

const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

// The manifest's text, from [name, sha256] pairs.
export const fleetManifest = (version, entries) => {
	if (!SEMVER.test(version)) throw new Error(`the version "${version}" isn't X.Y.Z`);
	if (entries.length === 0) throw new Error("the manifest needs at least one binary");
	const binaries = {};
	for (const [name, sum] of entries) {
		if (!TARGETS.some((target) => name === `maxd-${target}`))
			throw new Error(`"${name}" isn't maxd-<target> for one of ${TARGETS.join(", ")}`);
		if (name in binaries) throw new Error(`${name} is listed twice`);
		if (!/^[0-9a-f]{64}$/.test(sum)) throw new Error(`${name}'s sha256 isn't 64 hex digits`);
		binaries[name] = sum;
	}
	const sorted = Object.fromEntries(
		Object.entries(binaries).sort(([a], [b]) => a.localeCompare(b))
	);
	return `${JSON.stringify({ version, binaries: sorted }, null, 2)}\n`;
};

const main = () => {
	const { values, positionals } = parseArgs({
		options: { version: { type: "string" }, out: { type: "string" } },
		allowPositionals: true,
	});
	try {
		if (!values.version || !values.out) throw new Error("--version and --out are required");
		const entries = positionals.map((pair) => {
			const at = pair.indexOf("=");
			if (at < 1) throw new Error(`"${pair}" isn't name=path`);
			return [pair.slice(0, at), sha256(readFileSync(pair.slice(at + 1)))];
		});
		writeFileSync(values.out, fleetManifest(values.version, entries));
		for (const [name, sum] of entries) console.log(`${sum}  ${name}`);
	} catch (error) {
		console.error(`maxd-release.mjs: ${error.message}`);
		process.exit(1);
	}
};

if (import.meta.main) main();
