// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Reads and bumps the one version MaxHQ has: `version` under
// [workspace.package] in the root Cargo.toml. Every crate and the app inherit
// it, and tauri.conf.json has none of its own. Run `cargo update --workspace`
// after `set`, so Cargo.lock's entries for the workspace crates follow.
//
//   node scripts/release/version.mjs get              # prints 0.1.0
//   node scripts/release/version.mjs next patch       # prints 0.1.1, changes nothing
//   node scripts/release/version.mjs set 0.2.0        # writes Cargo.toml

import { readFileSync, writeFileSync } from "node:fs";

const CARGO_TOML = new URL("../../Cargo.toml", import.meta.url);

// ASCII digits without leading zeros, as semver has them.
const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

// The `version = "…"` line inside [workspace.package], up to the next table.
const sectionPattern = /^\[workspace\.package\][ \t]*$([\s\S]*?)(?=^\[|(?![\s\S]))/gm;
const versionPattern = /^(version[ \t]*=[ \t]*)"([^"\n]*)"/gm;

const versionLines = (text) => {
	const sections = [...text.matchAll(sectionPattern)];
	if (sections.length !== 1)
		throw new Error(`expected one [workspace.package] table, found ${sections.length}`);
	const [section] = sections;
	const start = section.index + section[0].length - section[1].length;
	return [...section[1].matchAll(versionPattern)].map((match) => ({
		start: start + match.index + match[1].length,
		end: start + match.index + match[0].length,
		value: match[2],
	}));
};

const theVersionLine = (text) => {
	const lines = versionLines(text);
	if (lines.length !== 1)
		throw new Error(`expected one version in [workspace.package], found ${lines.length}`);
	return lines[0];
};

export const readVersion = (text) => {
	const { value } = theVersionLine(text);
	if (!SEMVER.test(value)) throw new Error(`the version "${value}" isn't X.Y.Z`);
	return value;
};

export const nextVersion = (current, kind) => {
	const match = SEMVER.exec(current);
	if (!match) throw new Error(`the version "${current}" isn't X.Y.Z`);
	const [major, minor, patch] = match.slice(1).map(Number);
	switch (kind) {
		case "major":
			return `${major + 1}.0.0`;
		case "minor":
			return `${major}.${minor + 1}.0`;
		case "patch":
			return `${major}.${minor}.${patch + 1}`;
		default:
			throw new Error(`the bump is patch, minor or major, not "${kind}"`);
	}
};

// Returns the text with the new version, or throws without a change.
export const setVersion = (text, version) => {
	if (!SEMVER.test(version)) throw new Error(`the new version "${version}" isn't X.Y.Z`);
	const line = theVersionLine(text);
	return `${text.slice(0, line.start)}"${version}"${text.slice(line.end)}`;
};

const main = () => {
	const [command, argument] = process.argv.slice(2);
	const text = readFileSync(CARGO_TOML, "utf8");
	try {
		if (command === "get" && argument === undefined) {
			console.log(readVersion(text));
		} else if (command === "next" && argument) {
			console.log(nextVersion(readVersion(text), argument));
		} else if (command === "set" && argument) {
			const before = readVersion(text);
			writeFileSync(CARGO_TOML, setVersion(text, argument));
			console.error(`Cargo.toml: ${before} -> ${argument}`);
		} else {
			console.error("usage: version.mjs get | next patch|minor|major | set X.Y.Z");
			process.exit(2);
		}
	} catch (error) {
		console.error(`version.mjs: ${error.message}. Nothing was changed.`);
		process.exit(1);
	}
};

if (import.meta.main) main();
