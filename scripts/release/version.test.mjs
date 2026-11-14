// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { nextVersion, readVersion, setVersion } from "./version.mjs";

const cargo = `[workspace]
members = ["crates/a"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
other = "2"
`;

test("reads the workspace version and nothing else", () => {
	assert.equal(readVersion(cargo), "0.1.0");
});

test("reads the repo's own Cargo.toml", () => {
	const text = readFileSync(new URL("../../Cargo.toml", import.meta.url), "utf8");
	assert.match(readVersion(text), /^\d+\.\d+\.\d+$/);
});

test("next version for each bump", () => {
	assert.equal(nextVersion("0.1.9", "patch"), "0.1.10");
	assert.equal(nextVersion("0.9.3", "minor"), "0.10.0");
	assert.equal(nextVersion("1.2.3", "major"), "2.0.0");
	assert.throws(() => nextVersion("1.2.3", "huge"), /patch, minor or major/);
	assert.throws(() => nextVersion("1.2", "patch"), /isn't X.Y.Z/);
});

test("set changes only the workspace version", () => {
	const out = setVersion(cargo, "0.2.0");
	assert.equal(out, cargo.replace('version = "0.1.0"', 'version = "0.2.0"'));
	assert.equal(readVersion(out), "0.2.0");
});

test("set refuses a version that isn't semver", () => {
	for (const bad of ["1.2", "01.2.3", "1.2.3-beta", "v1.2.3", ""]) {
		assert.throws(() => setVersion(cargo, bad), /isn't X.Y.Z/, bad);
	}
});

test("refuses unless the version is there exactly once", () => {
	const none = cargo.replace('version = "0.1.0"\n', "");
	assert.throws(() => setVersion(none, "0.2.0"), /found 0/);
	const twice = cargo.replace('version = "0.1.0"', 'version = "0.1.0"\nversion = "0.1.1"');
	assert.throws(() => setVersion(twice, "0.2.0"), /found 2/);
	const noTable = cargo.replace("[workspace.package]", "[package]");
	assert.throws(() => readVersion(noTable), /one \[workspace.package\] table, found 0/);
	assert.throws(() => readVersion(`${cargo}\n[workspace.package]\n`), /found 2/);
});

test("a workspace.package table at the end of the file", () => {
	const last = `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "3.4.5"`;
	assert.equal(readVersion(last), "3.4.5");
	assert.equal(setVersion(last, "3.4.6").endsWith('version = "3.4.6"'), true);
});
