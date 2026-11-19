// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Writes mobile.json, the manifest the iPhone app fetches from the newest
// release to learn of a newer TestFlight build: the version, the build
// number, the protocol range the build speaks, the release notes link and
// the date. Given the release notes, it also adds a line naming the iPhone
// build to them.
//
//   node scripts/release/mobile-json.mjs --version 0.2.0 --build 10702 \
//     --repo <owner>/<repo> --out mobile.json [--notes-file <file>]
//
// The protocol range comes from crates/protocol/src/lib.rs in the checkout,
// which is the tag the iPhone build came from.

import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;
const PROTOCOL_FILE = new URL("../../crates/protocol/src/lib.rs", import.meta.url);

const constant = (text, name) => {
	const match = new RegExp(`pub const ${name}: u32 = ([0-9]+);`).exec(text);
	if (!match) throw new Error(`${name} isn't in the protocol crate`);
	return Number(match[1]);
};

// The range the protocol crate's lib.rs declares.
export const readProtocol = (text) => ({
	current: constant(text, "PROTOCOL_VERSION"),
	oldest: constant(text, "OLDEST_PROTOCOL_VERSION"),
});

export const mobileJson = ({ version, build, protocol, repo, now = new Date() }) => {
	if (!SEMVER.test(version)) throw new Error(`the version "${version}" isn't X.Y.Z`);
	if (!Number.isSafeInteger(build) || build < 1)
		throw new Error(`the build "${build}" isn't a whole number above 0`);
	const { current, oldest } = protocol;
	if (!Number.isSafeInteger(current) || !Number.isSafeInteger(oldest) || oldest > current)
		throw new Error(`the protocol range ${oldest} to ${current} isn't valid`);
	if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error(`"${repo}" isn't <owner>/<repo>`);
	return `${JSON.stringify(
		{
			version,
			build,
			protocol: { current, oldest },
			notes_url: `https://github.com/${repo}/releases/tag/v${version}`,
			pub_date: now.toISOString().replace(/\.\d{3}Z$/, "Z"),
		},
		null,
		2
	)}\n`;
};

export const phoneLine = (version, build) =>
	`iPhone: MaxHQ ${version} (build ${build}) is in TestFlight.`;

// The notes with the iPhone line at the end, once, so a rerun adds nothing.
export const withPhoneLine = (notes, version, build) => {
	const line = phoneLine(version, build);
	const trimmed = notes.trimEnd();
	if (trimmed.split("\n").includes(line)) return `${trimmed}\n`;
	return trimmed ? `${trimmed}\n\n${line}\n` : `${line}\n`;
};

const main = () => {
	const { values } = parseArgs({
		options: {
			version: { type: "string" },
			build: { type: "string" },
			repo: { type: "string" },
			out: { type: "string" },
			"notes-file": { type: "string" },
		},
	});
	try {
		if (!values.version || !values.build || !values.repo || !values.out)
			throw new Error("--version, --build, --repo and --out are required");
		if (!/^[0-9]+$/.test(values.build))
			throw new Error(`"${values.build}" isn't a build number`);
		const build = Number(values.build);
		const protocol = readProtocol(readFileSync(PROTOCOL_FILE, "utf8"));
		const text = mobileJson({ version: values.version, build, protocol, repo: values.repo });
		writeFileSync(values.out, text);
		const notesFile = values["notes-file"];
		if (notesFile) {
			writeFileSync(
				notesFile,
				withPhoneLine(readFileSync(notesFile, "utf8"), values.version, build)
			);
		}
		console.log(
			`mobile.json for ${values.version} (${build}), protocol ${protocol.oldest} to ${protocol.current}`
		);
	} catch (error) {
		console.error(`mobile-json.mjs: ${error.message}`);
		process.exit(1);
	}
};

if (import.meta.main) main();
