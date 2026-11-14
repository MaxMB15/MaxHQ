// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Signs a file with the update key through `tauri signer sign`, which writes
// <file>.sig: the minisign signature, base64-encoded. The app's updater and
// maxd's fleet check both read that format.
//
// The key comes from TAURI_SIGNING_PRIVATE_KEY, or from the file that
// TAURI_SIGNING_PRIVATE_KEY_PATH names (for local builds). Its optional
// password comes from TAURI_SIGNING_PRIVATE_KEY_PASSWORD. The secret may hold
// the minisign key as plain text, with real or escaped newlines, or
// base64-encoded once or twice. The Tauri CLI takes it base64-encoded once, so
// this converts it and hands it over in the CLI's environment only. The key
// never goes on a command line, into a file, or into GITHUB_ENV.
//
//   node scripts/release/sign.mjs <file>
//   node scripts/release/sign.mjs --check     # only checks that the key reads

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const MARKER = "untrusted comment:";
const TAURI = fileURLToPath(new URL("../../apps/desktop/node_modules/.bin/tauri", import.meta.url));

const strictBase64 = (text) => {
	const compact = text.replace(/\s+/g, "");
	if (!compact || compact.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(compact))
		return null;
	try {
		return new TextDecoder("utf-8", { fatal: true }).decode(Buffer.from(compact, "base64"));
	} catch {
		return null;
	}
};

// The minisign secret key as plain text, whichever form the secret is in.
export const plainKey = (secret) => {
	let key = secret.replaceAll("\\n", "\n").trim();
	for (let round = 0; round < 3; round++) {
		if (key.startsWith(MARKER)) {
			const [comment] = key.split("\n", 1);
			if (!/secret key/i.test(comment))
				throw new Error("the key is a public key, not the private one");
			return `${key.replace(/\n+$/, "")}\n`;
		}
		const decoded = strictBase64(key);
		if (decoded === null) break;
		key = decoded.trim();
	}
	throw new Error("the key is not a minisign private key");
};

// What `tauri signer sign` takes: the plain key, base64-encoded once.
export const cliKey = (secret) => Buffer.from(plainKey(secret)).toString("base64");

const secretFromEnv = (env) => {
	if (env.TAURI_SIGNING_PRIVATE_KEY?.trim()) return env.TAURI_SIGNING_PRIVATE_KEY;
	const path = env.TAURI_SIGNING_PRIVATE_KEY_PATH;
	if (path) return readFileSync(path, "utf8");
	throw new Error(
		"set TAURI_SIGNING_PRIVATE_KEY, or TAURI_SIGNING_PRIVATE_KEY_PATH to a key file"
	);
};

const fail = (message) => {
	const prefix = process.env.GITHUB_ACTIONS ? "::error::" : "sign.mjs: ";
	console.error(`${prefix}${message}`);
	process.exit(1);
};

const main = () => {
	const args = process.argv.slice(2);
	if (args.length !== 1) fail("usage: sign.mjs <file> | --check");
	let key;
	try {
		key = cliKey(secretFromEnv(process.env));
	} catch (error) {
		fail(`${error.message}.`);
	}
	if (args[0] === "--check") {
		console.log("The update signing key is a minisign private key.");
		return;
	}
	const file = resolve(args[0]);
	if (!existsSync(file) || !statSync(file).isFile()) fail(`${file} doesn't exist.`);
	if (!existsSync(TAURI)) fail(`${TAURI} is missing. Run pnpm install first.`);

	const env = { ...process.env, TAURI_SIGNING_PRIVATE_KEY: key };
	delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
	// stdin closed, so a key with a password and none given fails instead of
	// waiting for one.
	const result = spawnSync(TAURI, ["signer", "sign", file], {
		env,
		stdio: ["ignore", "ignore", "inherit"],
	});
	if (result.error) fail(result.error.message);
	if (result.status !== 0) fail(`tauri signer sign exited with ${result.status}.`);
	if (!existsSync(`${file}.sig`)) fail(`tauri signer sign wrote no ${file}.sig.`);
	console.log(`Signed ${args[0]}`);
};

if (import.meta.main) main();
