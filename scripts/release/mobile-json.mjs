// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Writes mobile.json, the manifest the phone app fetches from the newest
// release to learn of a newer build: the version, the iPhone's TestFlight
// build number, the protocol range the build speaks, the release notes link
// and the date. With --android-build, also the Android build's version code
// and the link to the release's APK, which the phone installs from. Given
// the release notes, it adds a line naming each phone build to them.
//
//   node scripts/release/mobile-json.mjs --version 0.2.0 --build 10702 \
//     --repo <owner>/<repo> --out mobile.json [--android-build 10702] \
//     [--notes-file <file>]
//
// The APK is the release asset ANDROID_APK names. Leave --android-build out
// when the release has none, such as when it wasn't signed: the phone then
// keeps the Android build it has and offers the release's page.
//
// The protocol range comes from crates/protocol/src/lib.rs in the checkout,
// which is the tag the iPhone build came from.

import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

// The release's APK and AAB, without a version like the DMGs.
export const ANDROID_APK = "MaxHQ.apk";
export const ANDROID_AAB = "MaxHQ.aab";
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

const wholeBuild = (build) => {
	if (!Number.isSafeInteger(build) || build < 1)
		throw new Error(`the build "${build}" isn't a whole number above 0`);
	return build;
};

// `androidBuild` is the Android build's version code, or undefined when the
// release carries no APK.
export const mobileJson = ({ version, build, protocol, repo, androidBuild, now = new Date() }) => {
	if (!SEMVER.test(version)) throw new Error(`the version "${version}" isn't X.Y.Z`);
	wholeBuild(build);
	const { current, oldest } = protocol;
	if (!Number.isSafeInteger(current) || !Number.isSafeInteger(oldest) || oldest > current)
		throw new Error(`the protocol range ${oldest} to ${current} isn't valid`);
	if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error(`"${repo}" isn't <owner>/<repo>`);
	const android =
		androidBuild === undefined
			? {}
			: {
					android: {
						build: wholeBuild(androidBuild),
						apk_url: `https://github.com/${repo}/releases/download/v${version}/${ANDROID_APK}`,
					},
				};
	return `${JSON.stringify(
		{
			version,
			build,
			protocol: { current, oldest },
			notes_url: `https://github.com/${repo}/releases/tag/v${version}`,
			pub_date: now.toISOString().replace(/\.\d{3}Z$/, "Z"),
			...android,
		},
		null,
		2
	)}\n`;
};

export const phoneLine = (version, build) =>
	`iPhone: MaxHQ ${version} (build ${build}) is in TestFlight.`;

export const androidLine = (version, build) =>
	`Android: MaxHQ ${version} (build ${build}) is ${ANDROID_APK} on this release.`;

// The notes with a line at the end, once, so a rerun adds nothing.
export const withLine = (notes, line) => {
	const trimmed = notes.trimEnd();
	if (trimmed.split("\n").includes(line)) return `${trimmed}\n`;
	return trimmed ? `${trimmed}\n\n${line}\n` : `${line}\n`;
};

export const withPhoneLine = (notes, version, build) => withLine(notes, phoneLine(version, build));

const main = () => {
	const { values } = parseArgs({
		options: {
			version: { type: "string" },
			build: { type: "string" },
			repo: { type: "string" },
			out: { type: "string" },
			"android-build": { type: "string" },
			"notes-file": { type: "string" },
		},
	});
	try {
		if (!values.version || !values.build || !values.repo || !values.out)
			throw new Error("--version, --build, --repo and --out are required");
		const number = (flag, text) => {
			if (!/^[0-9]+$/.test(text)) throw new Error(`${flag} "${text}" isn't a build number`);
			return Number(text);
		};
		const build = number("--build", values.build);
		const androidBuild =
			values["android-build"] === undefined
				? undefined
				: number("--android-build", values["android-build"]);
		const { version, repo } = values;
		const protocol = readProtocol(readFileSync(PROTOCOL_FILE, "utf8"));
		writeFileSync(values.out, mobileJson({ version, build, protocol, repo, androidBuild }));
		const notesFile = values["notes-file"];
		if (notesFile) {
			let notes = withPhoneLine(readFileSync(notesFile, "utf8"), version, build);
			if (androidBuild !== undefined)
				notes = withLine(notes, androidLine(version, androidBuild));
			writeFileSync(notesFile, notes);
		}
		const android = androidBuild === undefined ? "no Android build" : `Android ${androidBuild}`;
		console.log(
			`mobile.json for ${version} (${build}, ${android}), protocol ${protocol.oldest} to ${protocol.current}`
		);
	} catch (error) {
		console.error(`mobile-json.mjs: ${error.message}`);
		process.exit(1);
	}
};

if (import.meta.main) main();
