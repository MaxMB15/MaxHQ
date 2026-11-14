// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { cliKey, plainKey } from "./sign.mjs";

const plain =
	"untrusted comment: rsign encrypted secret key\nRWRTY0Iy1234567890abcdefghijklmnopqrstuvwxyzABCDEFGHIJ==\n";
const once = Buffer.from(plain).toString("base64");
const twice = Buffer.from(once).toString("base64");

test("reads the key in every form a secret can hold it", () => {
	for (const [name, form] of [
		["plain", plain],
		["plain without the last newline", plain.trimEnd()],
		["escaped newlines", plain.replaceAll("\n", "\\n")],
		["base64 once", once],
		["base64 once with a newline", `${once}\n`],
		["base64 twice", twice],
	]) {
		assert.equal(plainKey(form), plain, name);
	}
});

test("hands the CLI the key base64-encoded once", () => {
	assert.equal(cliKey(twice), once);
});

test("refuses a public key and anything that isn't a key", () => {
	const pub = "untrusted comment: minisign public key: F66FD78C1FAC595C\nRWRcWawfjNdv9k7u\n";
	assert.throws(() => plainKey(pub), /public key/);
	assert.throws(() => plainKey(Buffer.from(pub).toString("base64")), /public key/);
	for (const bad of ["", "hello", "aGVsbG8=", "!!!", "Zm9v\u0000"]) {
		assert.throws(() => plainKey(bad), /not a minisign private key/, JSON.stringify(bad));
	}
});

const tauri = fileURLToPath(new URL("../../apps/desktop/node_modules/.bin/tauri", import.meta.url));
const script = fileURLToPath(new URL("./sign.mjs", import.meta.url));

test("signs a file with a throwaway key", { skip: !existsSync(tauri) }, () => {
	const dir = mkdtempSync(join(tmpdir(), "maxhq-sign-"));
	try {
		const keyPath = join(dir, "test.key");
		execFileSync(tauri, ["signer", "generate", "--ci", "-w", keyPath], { stdio: "ignore" });
		const file = join(dir, "maxd-release.json");
		writeFileSync(file, '{"version":"0.1.0"}\n');
		const env = { ...process.env, TAURI_SIGNING_PRIVATE_KEY_PATH: keyPath };
		delete env.TAURI_SIGNING_PRIVATE_KEY;
		delete env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;

		const check = spawnSync(process.execPath, [script, "--check"], { env, encoding: "utf8" });
		assert.equal(check.status, 0, check.stderr);

		// The secret as GitHub would hold it: the key file's text, plain.
		const plainEnv = {
			...env,
			TAURI_SIGNING_PRIVATE_KEY: plainKey(readFileSync(keyPath, "utf8")),
		};
		const run = spawnSync(process.execPath, [script, file], {
			env: plainEnv,
			encoding: "utf8",
		});
		assert.equal(run.status, 0, run.stderr);
		const signature = Buffer.from(readFileSync(`${file}.sig`, "utf8"), "base64").toString();
		assert.match(signature, /^untrusted comment: signature from tauri secret key\n/);
		assert.match(signature, /file:maxd-release\.json/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("fails without a key", () => {
	const env = { ...process.env };
	delete env.TAURI_SIGNING_PRIVATE_KEY;
	delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
	delete env.GITHUB_ACTIONS;
	const run = spawnSync(process.execPath, [script, "--check"], { env, encoding: "utf8" });
	assert.equal(run.status, 1);
	assert.match(run.stderr, /set TAURI_SIGNING_PRIVATE_KEY/);
});
