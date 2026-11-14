// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { latestJson } from "./latest-json.mjs";
import { fleetManifest, sha256 } from "./maxd-release.mjs";

const a = "a".repeat(64);
const b = "b".repeat(64);

test("the fleet manifest has the contract's shape, keys sorted", () => {
	const text = fleetManifest("0.2.0", [
		["maxd-x86_64-unknown-linux-musl", b],
		["maxd-aarch64-apple-darwin", a],
	]);
	assert.deepEqual(JSON.parse(text), {
		version: "0.2.0",
		binaries: { "maxd-aarch64-apple-darwin": a, "maxd-x86_64-unknown-linux-musl": b },
	});
	assert.equal(Object.keys(JSON.parse(text).binaries)[0], "maxd-aarch64-apple-darwin");
	assert.ok(text.endsWith("}\n"));
});

test("the fleet manifest refuses unknown names, duplicates and bad sums", () => {
	assert.throws(() => fleetManifest("0.2.0", [["maxd", a]]), /maxd-<target>/);
	assert.throws(() => fleetManifest("0.2.0", [["maxd-x86_64-pc-windows-msvc", a]]), /target/);
	assert.throws(
		() =>
			fleetManifest("0.2.0", [
				["maxd-aarch64-apple-darwin", a],
				["maxd-aarch64-apple-darwin", b],
			]),
		/twice/
	);
	assert.throws(() => fleetManifest("0.2.0", [["maxd-aarch64-apple-darwin", "abc"]]), /64 hex/);
	assert.throws(() => fleetManifest("0.2", [["maxd-aarch64-apple-darwin", a]]), /X.Y.Z/);
	assert.throws(() => fleetManifest("0.2.0", []), /at least one/);
});

test("sha256 matches a known sum", () => {
	assert.equal(
		sha256(Buffer.from("abc")),
		"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
	);
});

test("the fleet manifest script hashes the files it's given", () => {
	const dir = mkdtempSync(join(tmpdir(), "maxhq-manifest-"));
	try {
		writeFileSync(join(dir, "maxd"), "abc");
		const out = join(dir, "maxd-release.json");
		const script = fileURLToPath(new URL("./maxd-release.mjs", import.meta.url));
		const run = spawnSync(
			process.execPath,
			[
				script,
				"--version",
				"1.2.3",
				"--out",
				out,
				`maxd-aarch64-apple-darwin=${join(dir, "maxd")}`,
			],
			{ encoding: "utf8" }
		);
		assert.equal(run.status, 0, run.stderr);
		assert.deepEqual(JSON.parse(readFileSync(out, "utf8")).binaries, {
			"maxd-aarch64-apple-darwin": sha256(Buffer.from("abc")),
		});
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("latest.json has Tauri's static format", () => {
	const text = latestJson({
		version: "0.2.0",
		repo: "owner/MaxHQ",
		notes: "  Fixes.\n",
		signatures: { aarch64: "c2lnLWFybQ==\n", x64: "c2lnLWludGVs" },
		now: new Date("2026-11-14T10:11:12.345Z"),
	});
	assert.deepEqual(JSON.parse(text), {
		version: "0.2.0",
		notes: "Fixes.",
		pub_date: "2026-11-14T10:11:12Z",
		platforms: {
			"darwin-aarch64": {
				signature: "c2lnLWFybQ==",
				url: "https://github.com/owner/MaxHQ/releases/download/v0.2.0/MaxHQ_0.2.0_aarch64.app.tar.gz",
			},
			"darwin-x86_64": {
				signature: "c2lnLWludGVs",
				url: "https://github.com/owner/MaxHQ/releases/download/v0.2.0/MaxHQ_0.2.0_x64.app.tar.gz",
			},
		},
	});
});

test("latest.json refuses empty signatures and unknown arches", () => {
	const base = { version: "0.2.0", repo: "owner/MaxHQ" };
	assert.throws(() => latestJson({ ...base, signatures: { aarch64: " " } }), /empty/);
	assert.throws(() => latestJson({ ...base, signatures: { arm64: "x" } }), /aarch64 or x64/);
	assert.throws(() => latestJson({ ...base, signatures: {} }), /no signatures/);
	assert.throws(
		() => latestJson({ ...base, repo: "not a repo", signatures: { x64: "x" } }),
		/owner/
	);
});
