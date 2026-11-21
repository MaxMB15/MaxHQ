// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Signs a file with the update key the way `tauri signer sign` does, and
// writes <file>.sig: the minisign signature, base64-encoded. The app's
// updater and maxd's checks both read that format. It needs Node and nothing
// else, so the release's signing job runs it without installing a package.
//
// The key comes from TAURI_SIGNING_PRIVATE_KEY, or from the file that
// TAURI_SIGNING_PRIVATE_KEY_PATH names (for local builds). Its password, if
// it has one, comes from TAURI_SIGNING_PRIVATE_KEY_PASSWORD. With --next the
// same three names end in _NEXT instead: the key a key change moves to (see
// .github/RELEASING.md). The secret may hold the minisign key as plain text,
// with real or escaped newlines, or base64-encoded once or twice. The key
// never goes on a command line, into a file, or into GITHUB_ENV.
//
//   node scripts/release/sign.mjs [--next] [--app-version V] [--out F] <file>
//   node scripts/release/sign.mjs [--next] --check [--expect PUBKEY]
//
// --app-version puts `version:V` in the signature's trusted comment, as
// `tauri signer sign --app-version` does, so an updater that requires a
// signed version installs the file only as version V. --out writes the
// signature somewhere other than <file>.sig. --check only checks that the key
// and its password read, and with --expect that the key is the private half
// of PUBKEY (base64, as tauri.conf.json holds it).

import {
	createPrivateKey,
	createPublicKey,
	createHash,
	scryptSync,
	sign as ed25519,
} from "node:crypto";
import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { parseArgs } from "node:util";

const MARKER = "untrusted comment:";
const UNTRUSTED = "untrusted comment: signature from tauri secret key";
// Ed25519 over a BLAKE2b-512 hash of the file, which minisign calls ED.
const PREHASHED = Buffer.from("ED");
// The DER prefix of a PKCS #8 Ed25519 private key, before its 32-byte seed.
const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

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

// libsodium's scrypt parameters for an opslimit and memlimit, as minisign
// derives them to encrypt a secret key.
const scryptParams = (opslimit, memlimit) => {
	const r = 8;
	const ops = Math.max(32768, opslimit);
	let logN = 1;
	let p;
	if (ops < memlimit / 32) {
		p = 1;
		const maxN = ops / (r * 4);
		while (logN < 63 && 2 ** logN <= maxN / 2) logN++;
	} else {
		const maxN = memlimit / (r * 128);
		while (logN < 63 && 2 ** logN <= maxN / 2) logN++;
		const maxRp = Math.min(0x3fffffff, Math.floor(ops / 4 / 2 ** logN));
		p = Math.min(0x3fffffff, Math.floor(maxRp / r));
	}
	return { N: 2 ** logN, r, p };
};

// Reads a minisign secret key box: its key ID and Ed25519 seed. A wrong
// password or a damaged key fails here, since the public half the key holds
// must match the one its seed makes.
export const readKey = (secret, password = "") => {
	const [, line = ""] = plainKey(secret).split("\n");
	const box = Buffer.from(line.trim(), "base64");
	if (box.length !== 158 || box.subarray(0, 2).toString("latin1") !== "Ed")
		throw new Error("the key is not an Ed25519 minisign private key");
	const kdf = box.subarray(2, 4).toString("latin1");
	const sealed = box.subarray(54);
	let keynum;
	if (kdf === "Sc") {
		const opslimit = Number(box.readBigUInt64LE(38));
		const memlimit = Number(box.readBigUInt64LE(46));
		const { N, r, p } = scryptParams(opslimit, memlimit);
		if (128 * N * r * p > 1 << 30) throw new Error("the key asks for too much memory to open");
		const stream = scryptSync(Buffer.from(password, "utf8"), box.subarray(6, 38), 104, {
			N,
			r,
			p,
			maxmem: 2 * 128 * N * r * p,
		});
		keynum = Buffer.alloc(104);
		for (let i = 0; i < 104; i++) keynum[i] = sealed[i] ^ stream[i];
		stream.fill(0);
	} else if (kdf === "\0\0") {
		keynum = Buffer.from(sealed);
	} else {
		throw new Error("the key is encrypted in a way minisign doesn't know");
	}
	const keyId = Buffer.from(keynum.subarray(0, 8));
	const seed = Buffer.from(keynum.subarray(8, 40));
	const held = Buffer.from(keynum.subarray(40, 72));
	keynum.fill(0);
	const privateKey = createPrivateKey({
		key: Buffer.concat([PKCS8_PREFIX, seed]),
		format: "der",
		type: "pkcs8",
	});
	seed.fill(0);
	const publicKey = createPublicKey(privateKey)
		.export({ format: "der", type: "spki" })
		.subarray(-32);
	if (!publicKey.equals(held))
		throw new Error(
			password
				? "the key doesn't open with that password"
				: "the key has a password; set its _PASSWORD secret"
		);
	return { keyId, privateKey, publicKey };
};

// A key's ID as minisign prints it.
export const keyIdText = (keyId) => Buffer.from(keyId).reverse().toString("hex").toUpperCase();

// The key ID and Ed25519 public key in a public key, given base64-encoded as
// tauri.conf.json and `tauri signer generate` hold it.
export const readPublicKey = (base64) => {
	const text = strictBase64(base64);
	const line = text?.split("\n")[1]?.trim();
	const box = line ? Buffer.from(line, "base64") : Buffer.alloc(0);
	if (box.length !== 42 || box.subarray(0, 2).toString("latin1") !== "Ed")
		throw new Error("that is not a minisign public key");
	return { keyId: box.subarray(2, 10), publicKey: box.subarray(10) };
};

// The .sig file's text for `data`: the minisign signature box, signed over
// its trusted comment too, then base64-encoded.
export const signature = (key, data, { file, version, timestamp }) => {
	const hash = createHash("blake2b512").update(data).digest();
	const sig = ed25519(null, hash, key.privateKey);
	let trusted = `timestamp:${timestamp}\tfile:${file}`;
	if (version) trusted += `\tversion:${version}`;
	if (/[\r\n]/.test(trusted)) throw new Error("a file name or version can't hold a line break");
	const global = ed25519(null, Buffer.concat([sig, Buffer.from(trusted)]), key.privateKey);
	const box = [
		UNTRUSTED,
		Buffer.concat([PREHASHED, key.keyId, sig]).toString("base64"),
		`trusted comment: ${trusted}`,
		global.toString("base64"),
		"",
	].join("\n");
	return Buffer.from(box).toString("base64");
};

const keyFromEnv = (env, next) => {
	const name = next ? "TAURI_SIGNING_PRIVATE_KEY_NEXT" : "TAURI_SIGNING_PRIVATE_KEY";
	let secret = env[name];
	if (!secret?.trim()) {
		const path = env[`${name}_PATH`];
		if (!path) throw new Error(`set ${name}, or ${name}_PATH to a key file`);
		secret = readFileSync(path, "utf8");
	}
	return readKey(secret, env[`${name}_PASSWORD`] ?? "");
};

const fail = (message) => {
	const prefix = process.env.GITHUB_ACTIONS ? "::error::" : "sign.mjs: ";
	console.error(`${prefix}${message}`);
	process.exit(1);
};

const main = () => {
	let values, positionals;
	try {
		({ values, positionals } = parseArgs({
			allowPositionals: true,
			options: {
				check: { type: "boolean" },
				next: { type: "boolean" },
				expect: { type: "string" },
				"app-version": { type: "string" },
				out: { type: "string" },
			},
		}));
	} catch (error) {
		fail(error.message);
	}
	if (values.check ? positionals.length !== 0 : positionals.length !== 1)
		fail("usage: sign.mjs [--next] [--app-version V] [--out F] <file> | [--next] --check");
	const which = values.next ? "next update key" : "update key";
	let key;
	try {
		key = keyFromEnv(process.env, values.next);
	} catch (error) {
		fail(`The ${which}: ${error.message}.`);
	}
	if (values.check) {
		if (values.expect !== undefined) {
			let expected;
			try {
				expected = readPublicKey(values.expect);
			} catch (error) {
				fail(`The public key to check the ${which} against: ${error.message}.`);
			}
			if (!expected.publicKey.equals(key.publicKey))
				fail(
					`The ${which} is ${keyIdText(key.keyId)}, not the private half of ${keyIdText(expected.keyId)}.`
				);
		}
		console.log(`The ${which} reads: ${keyIdText(key.keyId)}.`);
		return;
	}
	const file = resolve(positionals[0]);
	if (!existsSync(file) || !statSync(file).isFile()) fail(`${file} doesn't exist.`);
	const out = values.out ? resolve(values.out) : `${file}.sig`;
	try {
		const text = signature(key, readFileSync(file), {
			file: basename(file),
			version: values["app-version"],
			timestamp: Math.floor(Date.now() / 1000),
		});
		writeFileSync(out, text);
	} catch (error) {
		fail(error.message);
	}
	console.log(`Signed ${positionals[0]} with the ${which}`);
};

if (import.meta.main) main();
