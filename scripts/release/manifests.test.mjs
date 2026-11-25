// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
	BUNDLES,
	DESKTOP_TARGETS,
	assetName,
	desktopAssets,
	maxdAsset,
	targetInfo,
} from "./bundles.mjs";
import { latestJson, readDesktopSigs, signatureKeyId } from "./latest-json.mjs";
import { fleetManifest, sha256 } from "./maxd-release.mjs";
import {
	androidLine,
	mobileJson,
	phoneLine,
	readProtocol,
	withLine,
	withPhoneLine,
	withoutPhoneLines,
} from "./mobile-json.mjs";

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
	assert.throws(() => fleetManifest("0.2.0", [["maxd-x86_64-pc-windows-gnu", a]]), /target/);
	assert.throws(() => fleetManifest("0.2.0", [["maxd-x86_64-pc-windows-msvc.exe", a]]), /target/);
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

test("the fleet manifest takes the Windows maxd by its target, without .exe", () => {
	const text = fleetManifest("0.2.0", [
		["maxd-x86_64-pc-windows-msvc", a],
		["maxd-aarch64-pc-windows-msvc", b],
		["maxd-x86_64-unknown-linux-musl", a],
	]);
	assert.deepEqual(Object.keys(JSON.parse(text).binaries), [
		"maxd-aarch64-pc-windows-msvc",
		"maxd-x86_64-pc-windows-msvc",
		"maxd-x86_64-unknown-linux-musl",
	]);
	for (const { target, os } of DESKTOP_TARGETS)
		if (os === "windows")
			assert.doesNotThrow(() => fleetManifest("0.2.0", [[maxdAsset(target), a]]));
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

test("the desktop installers have one name each and the updater's platform keys", () => {
	const assets = desktopAssets();
	assert.deepEqual(
		assets.map(({ asset }) => asset),
		[
			"MaxHQ_x64-setup.exe",
			"MaxHQ_x64.msi",
			"MaxHQ_aarch64-setup.exe",
			"MaxHQ_aarch64.msi",
			"MaxHQ_x64.AppImage",
			"MaxHQ_x64.deb",
			"MaxHQ_x64.rpm",
			"MaxHQ_aarch64.AppImage",
			"MaxHQ_aarch64.deb",
			"MaxHQ_aarch64.rpm",
		]
	);
	const keys = assets.flatMap(({ platforms }) => platforms);
	assert.equal(new Set(keys).size, keys.length);
	const byKey = Object.fromEntries(
		assets.flatMap(({ asset, platforms }) => platforms.map((key) => [key, asset]))
	);
	// A plain <os>-<cpu> key is what an app without a known bundle type
	// looks up, and that's the NSIS and AppImage builds.
	assert.equal(byKey["windows-x86_64"], "MaxHQ_x64-setup.exe");
	assert.equal(byKey["windows-x86_64-nsis"], "MaxHQ_x64-setup.exe");
	assert.equal(byKey["windows-aarch64-msi"], "MaxHQ_aarch64.msi");
	assert.equal(byKey["linux-aarch64"], "MaxHQ_aarch64.AppImage");
	assert.equal(byKey["linux-x86_64-appimage"], "MaxHQ_x64.AppImage");
	assert.equal(byKey["linux-x86_64-deb"], "MaxHQ_x64.deb");
	assert.equal(byKey["linux-aarch64-rpm"], "MaxHQ_aarch64.rpm");
	assert.equal(Object.keys(byKey).length, 14);
	assert.deepEqual(targetInfo("aarch64-unknown-linux-gnu").bundles, BUNDLES.linux);
	assert.throws(() => targetInfo("x86_64-unknown-linux-musl"), /isn't a desktop target/);
	assert.throws(() => assetName("x64", "dmg"), /isn't a desktop bundle type/);
	assert.equal(maxdAsset("aarch64-pc-windows-msvc"), "maxd-aarch64-pc-windows-msvc");
});

test("the bundles script lists the assets for the shell", () => {
	const script = fileURLToPath(new URL("./bundles.mjs", import.meta.url));
	const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
	assert.deepEqual(run("assets", "linux").stdout.trim().split("\n"), [
		"MaxHQ_x64.AppImage",
		"MaxHQ_x64.deb",
		"MaxHQ_x64.rpm",
		"MaxHQ_aarch64.AppImage",
		"MaxHQ_aarch64.deb",
		"MaxHQ_aarch64.rpm",
	]);
	assert.equal(run("assets").stdout.trim().split("\n").length, 10);
	assert.deepEqual(run("maxd").stdout.trim().split("\n"), [
		"maxd-x86_64-pc-windows-msvc",
		"maxd-aarch64-pc-windows-msvc",
	]);
	assert.equal(run("other").status, 2);
});

test("latest.json points each installer's platform keys at its release asset", () => {
	const json = JSON.parse(
		latestJson({
			version: "0.2.0",
			repo: "owner/MaxHQ",
			signatures: { aarch64: "c2lnLWFybQ==" },
			desktop: { "MaxHQ_x64.deb": "ZGVi\n", "MaxHQ_x64-setup.exe": "bnNpcw==" },
			now: new Date("2026-11-14T10:11:12Z"),
		})
	);
	const release = "https://github.com/owner/MaxHQ/releases/download/v0.2.0";
	assert.deepEqual(json.platforms, {
		"darwin-aarch64": {
			signature: "c2lnLWFybQ==",
			url: `${release}/MaxHQ_0.2.0_aarch64.app.tar.gz`,
		},
		"windows-x86_64": { signature: "bnNpcw==", url: `${release}/MaxHQ_x64-setup.exe` },
		"windows-x86_64-nsis": { signature: "bnNpcw==", url: `${release}/MaxHQ_x64-setup.exe` },
		"linux-x86_64-deb": { signature: "ZGVi", url: `${release}/MaxHQ_x64.deb` },
	});
	const base = { version: "0.2.0", repo: "owner/MaxHQ", signatures: {} };
	assert.throws(
		() => latestJson({ ...base, desktop: { "MaxHQ_0.2.0_x64.deb": "x" } }),
		/isn't one of the desktop installers/
	);
	assert.throws(() => latestJson({ ...base, desktop: { "MaxHQ_x64.rpm": "\n" } }), /empty/);
	assert.doesNotThrow(() => latestJson({ ...base, desktop: { "MaxHQ_x64.rpm": "x" } }));
});

// A .sig file's text and a public key, as far as their key IDs go.
const box = (magic, id, size) =>
	Buffer.concat([Buffer.from(magic), id, Buffer.alloc(size)]).toString("base64");
const fakeSig = (id) =>
	Buffer.from(`untrusted comment: x\n${box("ED", id, 64)}\ntrusted comment: t\ng\n`).toString(
		"base64"
	);
const fakePub = (id) =>
	Buffer.from(`untrusted comment: minisign public key\n${box("Ed", id, 32)}\n`).toString(
		"base64"
	);

test("latest.json takes only signatures made with --key", () => {
	const current = Buffer.from("0102030405060708", "hex");
	const next = Buffer.from("1112131415161718", "hex");
	const base = { version: "0.2.0", repo: "owner/MaxHQ" };
	const both = { aarch64: fakeSig(next), x64: fakeSig(next) };
	assert.deepEqual(signatureKeyId(fakeSig(next)), next);
	assert.doesNotThrow(() => latestJson({ ...base, signatures: both, key: fakePub(next) }));
	assert.throws(
		() =>
			latestJson({
				...base,
				signatures: { ...both, x64: fakeSig(current) },
				key: fakePub(next),
			}),
		/x64 signature is by key 0807060504030201, not 1817161514131211/
	);
	assert.throws(
		() => latestJson({ ...base, signatures: { x64: "c2lnLWludGVs" }, key: fakePub(next) }),
		/isn't a minisign signature/
	);
	assert.throws(
		() => latestJson({ ...base, signatures: both, key: "not a key" }),
		/not a minisign public key/
	);
});

test("latest.json checks the installers' signatures against --key too", () => {
	const current = Buffer.from("0102030405060708", "hex");
	const next = Buffer.from("1112131415161718", "hex");
	const base = { version: "0.2.0", repo: "owner/MaxHQ", signatures: {}, key: fakePub(next) };
	assert.doesNotThrow(() => latestJson({ ...base, desktop: { "MaxHQ_x64.msi": fakeSig(next) } }));
	assert.throws(
		() => latestJson({ ...base, desktop: { "MaxHQ_x64.msi": fakeSig(current) } }),
		/MaxHQ_x64.msi signature is by key 0807060504030201/
	);
});

test("the installer signatures are read by suffix, the missing ones named", () => {
	const dir = mkdtempSync(join(tmpdir(), "maxhq-desktop-sigs-"));
	try {
		writeFileSync(join(dir, "MaxHQ_x64.AppImage.sig"), "YXBw");
		writeFileSync(join(dir, "MaxHQ_x64.AppImage.next.sig"), "bmV4dA==");
		writeFileSync(join(dir, "MaxHQ_x64.AppImage"), "not a signature");
		const plain = readDesktopSigs(dir);
		assert.deepEqual(plain.found, { "MaxHQ_x64.AppImage": "YXBw" });
		assert.equal(plain.missing.length, 9);
		assert.ok(!plain.missing.includes("MaxHQ_x64.AppImage"));
		assert.deepEqual(readDesktopSigs(dir, ".next.sig").found, {
			"MaxHQ_x64.AppImage": "bmV4dA==",
		});
		const script = fileURLToPath(new URL("./latest-json.mjs", import.meta.url));
		const out = join(dir, "latest-v2.json");
		writeFileSync(join(dir, "mac.sig"), "bWFj");
		const run = spawnSync(
			process.execPath,
			[
				script,
				...["--version", "1.2.3", "--repo", "owner/MaxHQ", "--out", out],
				...["--sig", `x64=${join(dir, "mac.sig")}`],
				...["--desktop-sigs", dir, "--sig-suffix", ".next.sig"],
			],
			{ encoding: "utf8", env: { ...process.env, GITHUB_ACTIONS: "" } }
		);
		assert.equal(run.status, 0, run.stderr);
		assert.match(run.stdout, /has no MaxHQ_x64-setup.exe: its .next.sig isn't there/);
		assert.match(run.stdout, /for 1.2.3: x64, MaxHQ_x64.AppImage/);
		assert.deepEqual(Object.keys(JSON.parse(readFileSync(out, "utf8")).platforms), [
			"darwin-x86_64",
			"linux-x86_64",
			"linux-x86_64-appimage",
		]);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("mobile.json names the build, its protocol range and the notes", () => {
	const text = mobileJson({
		version: "0.2.0",
		build: 10702,
		protocol: { current: 45, oldest: 20 },
		repo: "owner/MaxHQ",
		now: new Date("2026-11-16T10:11:12.345Z"),
	});
	assert.deepEqual(JSON.parse(text), {
		version: "0.2.0",
		build: 10702,
		protocol: { current: 45, oldest: 20 },
		notes_url: "https://github.com/owner/MaxHQ/releases/tag/v0.2.0",
		pub_date: "2026-11-16T10:11:12Z",
	});
	assert.ok(text.endsWith("}\n"));
});

test("mobile.json names the Android build and its APK when the release has one", () => {
	const json = JSON.parse(
		mobileJson({
			version: "0.2.0",
			build: 10702,
			androidBuild: 10703,
			protocol: { current: 45, oldest: 20 },
			repo: "owner/MaxHQ",
			now: new Date("2026-11-16T10:11:12Z"),
		})
	);
	assert.deepEqual(json.android, {
		build: 10703,
		apk_url: "https://github.com/owner/MaxHQ/releases/download/v0.2.0/MaxHQ.apk",
	});
	assert.equal(Object.keys(json).at(-1), "android");
	const base = {
		version: "0.2.0",
		build: 3,
		protocol: { current: 45, oldest: 20 },
		repo: "owner/MaxHQ",
	};
	assert.equal("android" in JSON.parse(mobileJson(base)), false);
	assert.throws(() => mobileJson({ ...base, androidBuild: 0 }), /whole number/);
	assert.throws(() => mobileJson({ ...base, androidBuild: "7" }), /whole number/);
});

test("mobile.json refuses a bad version, build, range or repository", () => {
	const base = {
		version: "0.2.0",
		build: 3,
		protocol: { current: 45, oldest: 20 },
		repo: "owner/MaxHQ",
	};
	assert.throws(() => mobileJson({ ...base, version: "0.2" }), /X.Y.Z/);
	assert.throws(() => mobileJson({ ...base, build: 0 }), /whole number/);
	assert.throws(() => mobileJson({ ...base, build: 1.5 }), /whole number/);
	assert.throws(() => mobileJson({ ...base, protocol: { current: 19, oldest: 20 } }), /range/);
	assert.throws(() => mobileJson({ ...base, repo: "not a repo" }), /owner/);
});

test("the protocol range comes from the protocol crate", () => {
	const text = [
		"/// The version.",
		"pub const PROTOCOL_VERSION: u32 = 45;",
		"pub const OLDEST_PROTOCOL_VERSION: u32 = 20;",
	].join("\n");
	assert.deepEqual(readProtocol(text), { current: 45, oldest: 20 });
	assert.throws(() => readProtocol("pub const PROTOCOL_VERSION: u32 = 45;"), /OLDEST/);
	const real = readFileSync(new URL("../../crates/protocol/src/lib.rs", import.meta.url), "utf8");
	const range = readProtocol(real);
	assert.ok(range.oldest <= range.current);
});

test("the release notes name the iPhone build once", () => {
	const line = phoneLine("0.2.0", 10702);
	assert.equal(line, "iPhone: MaxHQ 0.2.0 (build 10702) is in TestFlight.");
	const once = withPhoneLine("## What's changed\n* Fixes.\n", "0.2.0", 10702);
	assert.equal(once, `## What's changed\n* Fixes.\n\n${line}\n`);
	assert.equal(withPhoneLine(once, "0.2.0", 10702), once);
	assert.equal(withPhoneLine("", "0.2.0", 10702), `${line}\n`);
});

test("the release notes name the Android build once, after the iPhone's", () => {
	const line = androidLine("0.2.0", 10703);
	assert.equal(line, "Android: MaxHQ 0.2.0 (build 10703) is MaxHQ.apk on this release.");
	const both = withLine(withPhoneLine("* Fixes.\n", "0.2.0", 10702), line);
	assert.equal(both, `* Fixes.\n\n${phoneLine("0.2.0", 10702)}\n\n${line}\n`);
	assert.equal(withLine(withPhoneLine(both, "0.2.0", 10702), line), both);
});

test("the desktop manifests' notes leave out the phone builds' lines", () => {
	const notes = withLine(
		withPhoneLine("* Fixes.\n", "0.2.0", 10702),
		androidLine("0.2.0", 10703)
	);
	assert.equal(withoutPhoneLines(notes), "* Fixes.");
	const kept = "* iPhone: MaxHQ 0.2.0 (build 1) is in TestFlight.\nAndroid: MaxHQ is fixed.\n";
	assert.equal(withoutPhoneLines(kept), kept.trimEnd());
	const dir = mkdtempSync(join(tmpdir(), "maxhq-latest-notes-"));
	try {
		const notesFile = join(dir, "notes.md");
		const sig = join(dir, "arm.sig");
		const out = join(dir, "latest.json");
		writeFileSync(notesFile, notes);
		writeFileSync(sig, "c2lnLWFybQ==");
		const script = fileURLToPath(new URL("./latest-json.mjs", import.meta.url));
		const run = spawnSync(
			process.execPath,
			[
				script,
				...["--version", "0.2.0", "--repo", "owner/MaxHQ", "--sig", `aarch64=${sig}`],
				...["--notes-file", notesFile, "--out", out],
			],
			{ encoding: "utf8" }
		);
		assert.equal(run.status, 0, run.stderr);
		assert.equal(JSON.parse(readFileSync(out, "utf8")).notes, "* Fixes.");
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("the mobile.json script reads the range and adds the notes line", () => {
	const dir = mkdtempSync(join(tmpdir(), "maxhq-mobile-json-"));
	try {
		const out = join(dir, "mobile.json");
		const notes = join(dir, "notes.md");
		writeFileSync(notes, "Fixes.\n");
		const script = fileURLToPath(new URL("./mobile-json.mjs", import.meta.url));
		const args = ["--version", "1.2.3", "--build", "10101", "--repo", "owner/MaxHQ"];
		const run = spawnSync(
			process.execPath,
			[script, ...args, "--out", out, "--notes-file", notes],
			{ encoding: "utf8" }
		);
		assert.equal(run.status, 0, run.stderr);
		const json = JSON.parse(readFileSync(out, "utf8"));
		assert.equal(json.build, 10101);
		assert.ok(json.protocol.oldest <= json.protocol.current);
		assert.equal(readFileSync(notes, "utf8"), `Fixes.\n\n${phoneLine("1.2.3", 10101)}\n`);
		const bad = spawnSync(process.execPath, [script, ...args.slice(0, 2), "--out", out], {
			encoding: "utf8",
		});
		assert.equal(bad.status, 1);
		assert.match(bad.stderr, /--build/);
		const android = spawnSync(
			process.execPath,
			[script, ...args, "--android-build", "10102", "--out", out, "--notes-file", notes],
			{ encoding: "utf8" }
		);
		assert.equal(android.status, 0, android.stderr);
		assert.equal(JSON.parse(readFileSync(out, "utf8")).android.build, 10102);
		assert.equal(
			readFileSync(notes, "utf8"),
			`Fixes.\n\n${phoneLine("1.2.3", 10101)}\n\n${androidLine("1.2.3", 10102)}\n`
		);
		const wrong = spawnSync(
			process.execPath,
			[script, ...args, "--android-build", "x1", "--out", out],
			{ encoding: "utf8" }
		);
		assert.equal(wrong.status, 1);
		assert.match(wrong.stderr, /--android-build "x1" isn't a build number/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});
