// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { createHash, createPublicKey, verify } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { plainKey, readKey, readPublicKey, signature } from "./sign.mjs";

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

test("refuses a public key and anything that isn't a key", () => {
	const pub = "untrusted comment: minisign public key: F66FD78C1FAC595C\nRWRcWawfjNdv9k7u\n";
	assert.throws(() => plainKey(pub), /public key/);
	assert.throws(() => plainKey(Buffer.from(pub).toString("base64")), /public key/);
	for (const bad of ["", "hello", "aGVsbG8=", "!!!", "Zm9v\u0000"]) {
		assert.throws(() => plainKey(bad), /not a minisign private key/, JSON.stringify(bad));
	}
	assert.throws(() => readKey(plain), /not an Ed25519 minisign private key/);
});

const tauri = fileURLToPath(new URL("../../apps/desktop/node_modules/.bin/tauri", import.meta.url));
const script = fileURLToPath(new URL("./sign.mjs", import.meta.url));
const haveTauri = existsSync(tauri);
const dir = mkdtempSync(join(tmpdir(), "maxhq-sign-"));
after(() => rmSync(dir, { recursive: true, force: true }));

// The environment without any update key in it.
const bare = () => {
	const env = { ...process.env };
	for (const name of Object.keys(env)) {
		if (name.startsWith("TAURI_SIGNING_PRIVATE_KEY")) delete env[name];
	}
	delete env.GITHUB_ACTIONS;
	return env;
};

// A throwaway key from `tauri signer generate`, with `password` if given.
const keys = new Map();
const throwaway = (password) => {
	if (keys.has(password)) return keys.get(password);
	const path = join(dir, `key-${keys.size}`);
	const args = ["signer", "generate", "--ci", "-w", path];
	if (password) args.push("-p", password);
	execFileSync(tauri, args, { env: bare(), stdio: "ignore" });
	const key = {
		path,
		secret: readFileSync(path, "utf8"),
		pub: readFileSync(`${path}.pub`, "utf8"),
	};
	keys.set(password, key);
	return key;
};

// Checks a .sig file's text as minisign does: the signature over the file's
// BLAKE2b-512 hash, then the one over the signature and trusted comment.
const check = (sigText, data, pub) => {
	const lines = Buffer.from(sigText, "base64").toString().split("\n");
	const box = Buffer.from(lines[1], "base64");
	const { keyId, publicKey } = readPublicKey(pub);
	const der = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), publicKey]);
	const key = createPublicKey({ key: der, format: "der", type: "spki" });
	const sig = box.subarray(10);
	const trusted = lines[2].replace(/^trusted comment: /, "");
	assert.equal(box.subarray(0, 2).toString(), "ED");
	assert.ok(box.subarray(2, 10).equals(keyId), "the key ID");
	assert.ok(verify(null, createHash("blake2b512").update(data).digest(), key, sig), "the file");
	assert.ok(
		verify(
			null,
			Buffer.concat([sig, Buffer.from(trusted)]),
			key,
			Buffer.from(lines[3], "base64")
		),
		"the trusted comment"
	);
	return lines;
};

for (const password of ["", "correct horse"]) {
	const which = password ? "with a password" : "without one";
	test(`signs as tauri signer sign does, ${which}`, { skip: !haveTauri }, () => {
		const key = throwaway(password);
		const file = join(dir, "MaxHQ_1.2.3_aarch64.app.tar.gz");
		writeFileSync(file, `an archive ${which}\n`);
		execFileSync(tauri, ["signer", "sign", "-f", key.path, "--app-version", "1.2.3", file], {
			env: { ...bare(), TAURI_SIGNING_PRIVATE_KEY_PASSWORD: password },
			stdio: ["ignore", "ignore", "inherit"],
		});
		const theirs = check(readFileSync(`${file}.sig`, "utf8"), readFileSync(file), key.pub);

		// The secret as GitHub would hold it: the key file's text, plain.
		const run = spawnSync(process.execPath, [script, "--app-version", "1.2.3", file], {
			env: {
				...bare(),
				TAURI_SIGNING_PRIVATE_KEY: plainKey(key.secret),
				TAURI_SIGNING_PRIVATE_KEY_PASSWORD: password,
			},
			encoding: "utf8",
		});
		assert.equal(run.status, 0, run.stderr);
		const ours = check(readFileSync(`${file}.sig`, "utf8"), readFileSync(file), key.pub);
		const comment = (lines) => lines[2].replace(/timestamp:\d+/, "timestamp:T");
		assert.equal(ours[0], theirs[0]);
		assert.equal(comment(ours), comment(theirs));
		assert.match(
			ours[2],
			/^trusted comment: timestamp:\d+\tfile:MaxHQ_1\.2\.3_aarch64\.app\.tar\.gz\tversion:1\.2\.3$/
		);
		assert.equal(ours.length, 5);
		assert.equal(ours[4], "");
	});
}

test(
	"leaves the version out unless asked, and writes where --out says",
	{ skip: !haveTauri },
	() => {
		const key = throwaway("");
		const file = join(dir, "maxd-release.json");
		writeFileSync(file, '{"version":"0.1.0"}\n');
		const out = join(dir, "elsewhere.sig");
		const run = spawnSync(process.execPath, [script, "--next", "--out", out, file], {
			env: { ...bare(), TAURI_SIGNING_PRIVATE_KEY_NEXT_PATH: key.path },
			encoding: "utf8",
		});
		assert.equal(run.status, 0, run.stderr);
		assert.match(run.stdout, /with the next update key/);
		assert.equal(existsSync(`${file}.sig`), false);
		const lines = check(readFileSync(out, "utf8"), readFileSync(file), key.pub);
		assert.match(lines[2], /^trusted comment: timestamp:\d+\tfile:maxd-release\.json$/);
	}
);

test("opens a key only with its password", { skip: !haveTauri }, () => {
	const key = throwaway("correct horse");
	assert.throws(() => readKey(key.secret), /has a password/);
	assert.throws(() => readKey(key.secret, "wrong"), /doesn't open with that password/);
	assert.ok(
		readKey(key.secret, "correct horse").publicKey.equals(readPublicKey(key.pub).publicKey)
	);
});

test(
	"--check --expect tells the key's own public half from another's",
	{ skip: !haveTauri },
	() => {
		const mine = throwaway("");
		const other = throwaway("correct horse");
		const run = (expect, extra = {}) =>
			spawnSync(process.execPath, [script, "--check", "--expect", expect], {
				env: { ...bare(), TAURI_SIGNING_PRIVATE_KEY_PATH: mine.path, ...extra },
				encoding: "utf8",
			});
		const good = run(mine.pub);
		assert.equal(good.status, 0, good.stderr);
		assert.match(good.stdout, /The update key reads: [0-9A-F]{16}\./);
		const bad = run(other.pub);
		assert.equal(bad.status, 1);
		assert.match(bad.stderr, /not the private half of [0-9A-F]{16}/);
		const wrong = run(mine.pub, { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "nope" });
		assert.equal(wrong.status, 1);
		assert.match(wrong.stderr, /doesn't open with that password/);
		const notKey = run("aGVsbG8=");
		assert.equal(notKey.status, 1);
		assert.match(notKey.stderr, /not a minisign public key/);
	}
);

test("signs the same box for a fixed key and time", { skip: !haveTauri }, () => {
	const key = readKey(throwaway("").secret);
	const options = { file: "x", version: "1.0.0", timestamp: 1 };
	assert.equal(
		signature(key, Buffer.from("a"), options),
		signature(key, Buffer.from("a"), options)
	);
	assert.throws(
		() => signature(key, Buffer.from("a"), { ...options, version: "1\n2" }),
		/line break/
	);
});

test("fails without a key", () => {
	for (const [args, name] of [
		[["--check"], "TAURI_SIGNING_PRIVATE_KEY"],
		[["--next", "--check"], "TAURI_SIGNING_PRIVATE_KEY_NEXT"],
	]) {
		const run = spawnSync(process.execPath, [script, ...args], {
			env: bare(),
			encoding: "utf8",
		});
		assert.equal(run.status, 1);
		assert.match(run.stderr, new RegExp(`set ${name}, or ${name}_PATH`));
	}
});
