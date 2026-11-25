// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { endpoint, tauriConfig } from "./updater-config.mjs";

const repo = { MAXHQ_RELEASES_REPO: "owner/MaxHQ" };

test("the build config writes no updater artifacts and takes the caller's bundle keys", () => {
	const { config, notes, warning } = tauriConfig({});
	assert.deepEqual(config, { bundle: { createUpdaterArtifacts: false } });
	assert.deepEqual(notes, []);
	assert.match(warning, /can't look for updates/);
	assert.deepEqual(tauriConfig({}, { externalBin: ["binaries/maxd"] }).config, {
		bundle: { createUpdaterArtifacts: false, externalBin: ["binaries/maxd"] },
	});
});

test("the build config names the release's manifest and key", () => {
	const plain = tauriConfig(repo);
	assert.deepEqual(plain.config.plugins, {
		updater: {
			endpoints: ["https://github.com/owner/MaxHQ/releases/latest/download/latest.json"],
		},
	});
	assert.deepEqual(plain.notes, [
		"Updates come from https://github.com/owner/MaxHQ/releases/latest/download/latest.json",
	]);
	const change = tauriConfig({
		...repo,
		MAXHQ_UPDATE_MANIFEST: "latest-v2.json",
		MAXHQ_UPDATE_PUBKEY: "dW50cnVzdGVk+/=",
	});
	assert.deepEqual(change.config.plugins.updater, {
		endpoints: [endpoint("owner/MaxHQ", "latest-v2.json")],
		pubkey: "dW50cnVzdGVk+/=",
	});
	assert.equal(change.notes.length, 2);
	assert.equal(change.warning, undefined);
	assert.equal(
		tauriConfig({ ...repo, MAXHQ_UPDATE_MANIFEST: "" }).config.plugins.updater.endpoints[0],
		endpoint("owner/MaxHQ", "latest.json")
	);
});

test("the build config refuses a bad repository, manifest or key", () => {
	assert.throws(() => tauriConfig({ MAXHQ_RELEASES_REPO: "MaxHQ" }), /owner\/name, not MaxHQ/);
	assert.throws(
		() => tauriConfig({ ...repo, MAXHQ_UPDATE_MANIFEST: "mobile.json" }),
		/latest.json or latest-v<n>.json, not mobile.json/
	);
	assert.throws(
		() => tauriConfig({ ...repo, MAXHQ_UPDATE_PUBKEY: "a key" }),
		/isn't a base64 public key/
	);
	assert.throws(() => tauriConfig({ MAXHQ_UPDATE_PUBKEY: "abc" }), /needs MAXHQ_RELEASES_REPO/);
});

test("the command prints the config for package-macos.sh and the rest on stderr", () => {
	const script = fileURLToPath(new URL("./updater-config.mjs", import.meta.url));
	const run = (env) =>
		spawnSync(process.execPath, [script], {
			encoding: "utf8",
			env: { PATH: process.env.PATH, ...env },
		});
	const ok = run({ ...repo, MAXHQ_UPDATE_PUBKEY: "dW50cnVzdGVk+/=" });
	assert.equal(ok.status, 0);
	assert.deepEqual(
		JSON.parse(ok.stdout),
		tauriConfig({ ...repo, MAXHQ_UPDATE_PUBKEY: "dW50cnVzdGVk+/=" }).config
	);
	assert.match(ok.stderr, /^==> Updates come from https:\/\/github\.com\/owner\/MaxHQ\//);
	const none = run({ GITHUB_ACTIONS: "true" });
	assert.equal(none.status, 0);
	assert.match(none.stderr, /^::warning::MAXHQ_RELEASES_REPO isn't set/);
	const bad = run({ MAXHQ_RELEASES_REPO: "MaxHQ", GITHUB_ACTIONS: "true" });
	assert.equal(bad.status, 1);
	assert.equal(bad.stdout, "");
	assert.match(bad.stderr, /^::error::updater-config\.mjs: MAXHQ_RELEASES_REPO is owner\/name/);
});
