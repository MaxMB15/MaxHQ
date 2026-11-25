// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The steps the release and its dry run share: sign-desktop.sh,
// maxd-sums.sh, write-manifests.sh and `mobile.ts android-sign`, run as the
// workflows run them, and validate-release.sh, which checks a published
// release.

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFile, execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { desktopAssets, maxdAsset } from "./bundles.mjs";
import { latestJson, readDesktopSigs, signatureKeyId } from "./latest-json.mjs";
import { keyIdText, readPublicKey } from "./sign.mjs";
import { repoVersion as version } from "./version.mjs";

const here = (name) => fileURLToPath(new URL(name, import.meta.url));
const tauri = here("../../apps/desktop/node_modules/.bin/tauri");
const haveTauri = existsSync(tauri);
const dir = mkdtempSync(join(tmpdir(), "maxhq-steps-"));
after(() => rmSync(dir, { recursive: true, force: true }));

// The environment without any key or CI marker in it.
const bare = (extra = {}) => {
	const env = { ...process.env };
	for (const name of Object.keys(env))
		if (name.startsWith("TAURI_SIGNING_PRIVATE_KEY") || name.startsWith("ANDROID_KEY"))
			delete env[name];
	delete env.GITHUB_ACTIONS;
	delete env.MAXHQ_UPDATE_KEY;
	delete env.MAXHQ_BRIDGE;
	return { ...env, ...extra };
};

const sh = (script, args, env = bare()) =>
	spawnSync("sh", [here(script), ...args], { encoding: "utf8", env });

// apps/mobile/scripts/mobile.ts as the signing jobs run it: from the
// checkout, on Node alone.
const mobile = (args, env = bare()) =>
	spawnSync(
		process.execPath,
		[
			"--disable-warning=MODULE_TYPELESS_PACKAGE_JSON",
			here("../../apps/mobile/scripts/mobile.ts"),
			...args,
		],
		{ encoding: "utf8", env }
	);

const folder = (name) => {
	const path = join(dir, name);
	mkdirSync(path, { recursive: true });
	return path;
};

// A throwaway update key from `tauri signer generate`.
const throwaway = (name) => {
	const path = join(dir, name);
	execFileSync(tauri, ["signer", "generate", "--ci", "-w", path], {
		env: bare(),
		stdio: "ignore",
	});
	return { path, pub: readFileSync(`${path}.pub`, "utf8").trim() };
};

test(
	"sign-desktop.sh signs each installer there and warns about the rest",
	{ skip: !haveTauri },
	() => {
		const key = throwaway("current");
		const out = folder("desktop");
		const linux = desktopAssets().filter(({ target }) => target.includes("-linux-"));
		for (const { asset } of linux) writeFileSync(join(out, asset), `installer ${asset}`);
		writeFileSync(join(out, "MaxHQ_x64-setup.exe.sig"), "stale");
		const run = sh(
			"sign-desktop.sh",
			[out],
			bare({ TAURI_SIGNING_PRIVATE_KEY_PATH: key.path })
		);
		assert.equal(run.status, 0, run.stderr);
		assert.match(
			run.stdout,
			new RegExp(`Signed ${linux.length} installers for ${version()}\\.`)
		);
		assert.match(
			run.stderr,
			/warning: MaxHQ_x64-setup.exe isn't in .*, so it has no signature/
		);
		assert.ok(!existsSync(join(out, "MaxHQ_x64-setup.exe.sig")), "a stale .sig goes");
		const { found, missing } = readDesktopSigs(out);
		assert.deepEqual(
			Object.keys(found),
			linux.map(({ asset }) => asset)
		);
		assert.equal(missing.length, desktopAssets().length - linux.length);
		// Each signature names the version and goes into latest.json for its key.
		const sig = Buffer.from(found["MaxHQ_x64.deb"], "base64").toString();
		assert.match(
			sig,
			new RegExp(
				`trusted comment: timestamp:\\d+\\tfile:MaxHQ_x64\\.deb\\tversion:${version()}`
			)
		);
		const manifest = JSON.parse(
			latestJson({
				version: "0.2.0",
				repo: "owner/MaxHQ",
				signatures: {},
				desktop: found,
				key: key.pub,
			})
		);
		assert.deepEqual(Object.keys(manifest.platforms).sort(), [
			"linux-aarch64",
			"linux-aarch64-appimage",
			"linux-aarch64-deb",
			"linux-aarch64-rpm",
			"linux-x86_64",
			"linux-x86_64-appimage",
			"linux-x86_64-deb",
			"linux-x86_64-rpm",
		]);
	}
);

test(
	"sign-desktop.sh signs with both keys in the bridge release and with the next after it",
	{ skip: !haveTauri },
	() => {
		const current = throwaway("bridge-current");
		const next = throwaway("bridge-next");
		const keys = {
			TAURI_SIGNING_PRIVATE_KEY_PATH: current.path,
			TAURI_SIGNING_PRIVATE_KEY_NEXT_PATH: next.path,
		};
		const out = folder("bridge");
		writeFileSync(join(out, "MaxHQ_aarch64.AppImage"), "appimage");
		const bridge = sh("sign-desktop.sh", [out], bare({ ...keys, MAXHQ_BRIDGE: "1" }));
		assert.equal(bridge.status, 0, bridge.stderr);
		const id = (file) => signatureKeyId(readFileSync(join(out, file), "utf8"));
		assert.ok(id("MaxHQ_aarch64.AppImage.sig").equals(readPublicKey(current.pub).keyId));
		assert.ok(id("MaxHQ_aarch64.AppImage.next.sig").equals(readPublicKey(next.pub).keyId));

		const moved = sh("sign-desktop.sh", [out], bare({ ...keys, MAXHQ_UPDATE_KEY: "next" }));
		assert.equal(moved.status, 0, moved.stderr);
		assert.ok(id("MaxHQ_aarch64.AppImage.sig").equals(readPublicKey(next.pub).keyId));
		assert.ok(!existsSync(join(out, "MaxHQ_aarch64.AppImage.next.sig")));

		const both = sh(
			"sign-desktop.sh",
			[out],
			bare({ ...keys, MAXHQ_UPDATE_KEY: "next", MAXHQ_BRIDGE: "1" })
		);
		assert.equal(both.status, 1);
		assert.match(both.stderr, /can't go with MAXHQ_UPDATE_KEY=next/);
	}
);

test("sign-desktop.sh fails without a key or an installer", { skip: !haveTauri }, () => {
	const empty = folder("empty");
	const noKey = sh("sign-desktop.sh", [empty]);
	assert.equal(noKey.status, 1);
	assert.match(noKey.stderr, /the update key doesn't read/);
	const key = throwaway("lonely");
	const none = sh("sign-desktop.sh", [empty], bare({ TAURI_SIGNING_PRIVATE_KEY_PATH: key.path }));
	assert.equal(none.status, 1);
	assert.match(none.stderr, /has none of the installers: MaxHQ_x64-setup.exe /);
	const ci = sh(
		"sign-desktop.sh",
		[join(dir, "nowhere")],
		bare({ GITHUB_ACTIONS: "true", TAURI_SIGNING_PRIVATE_KEY_PATH: key.path })
	);
	assert.match(ci.stderr, /^::error::sign-desktop.sh: .*nowhere isn't a folder/);
	assert.match(sh("sign-desktop.sh", []).stderr, /usage: sign-desktop.sh <dir>/);
});

test("lib.sh's die and warn name the script and annotate on GitHub Actions", () => {
	const script = join(dir, "lib-user.sh");
	writeFileSync(
		script,
		`. ${JSON.stringify(here("lib.sh"))}\nwarn "it's $1"\n[ "$1" = fine ] || die "$1 failed"\necho done\n`
	);
	const local = spawnSync("sh", [script, "broken"], { encoding: "utf8", env: bare() });
	assert.equal(local.status, 1);
	assert.equal(local.stdout, "");
	assert.equal(local.stderr, "warning: it's broken\nlib-user.sh: broken failed\n");
	const ci = spawnSync("sh", [script, "broken"], {
		encoding: "utf8",
		env: bare({ GITHUB_ACTIONS: "true" }),
	});
	assert.equal(ci.stderr, "::warning::it's broken\n::error::lib-user.sh: broken failed\n");
	const fine = spawnSync("sh", [script, "fine"], { encoding: "utf8", env: bare() });
	assert.equal(fine.status, 0);
	assert.equal(fine.stdout, "done\n");
});

test("maxd-sums.sh sums the Linux maxd and each Windows one there", () => {
	const out = folder("maxd");
	const put = (name) => writeFileSync(join(out, name), `binary ${name}`);
	const sha = (name) => createHash("sha256").update(`binary ${name}`).digest("hex");
	put("maxd-x86_64-unknown-linux-musl");
	const short = sh("maxd-sums.sh", [out]);
	assert.equal(short.status, 1);
	assert.match(short.stderr, /has no maxd-aarch64-unknown-linux-musl/);

	put("maxd-aarch64-unknown-linux-musl");
	put(maxdAsset("x86_64-pc-windows-msvc"));
	const run = sh("maxd-sums.sh", [out], bare({ GITHUB_ACTIONS: "true" }));
	assert.equal(run.status, 0, run.stderr);
	const names = [
		"maxd-x86_64-unknown-linux-musl",
		"maxd-aarch64-unknown-linux-musl",
		"maxd-x86_64-pc-windows-msvc",
	];
	assert.deepEqual(run.stdout.trim().split("\n"), [...names, "SHA256SUMS"]);
	assert.match(
		run.stderr,
		/::warning::.* has no maxd-aarch64-pc-windows-msvc, so the release goes out without it/
	);
	assert.equal(
		readFileSync(join(out, "SHA256SUMS"), "utf8"),
		names.map((name) => `${sha(name)}  ${name}\n`).join("")
	);
});

test(
	"write-manifests.sh writes each key state's manifests, the installers only in the app's",
	{ skip: !haveTauri },
	() => {
		const current = throwaway("manifest-current");
		const next = throwaway("manifest-next");
		const keys = bare({
			TAURI_SIGNING_PRIVATE_KEY_PATH: current.path,
			TAURI_SIGNING_PRIVATE_KEY_NEXT_PATH: next.path,
		});
		const v = version();
		// The Macs' archives and the Linux installers, signed as the bridge
		// release signs them.
		const mac = folder("manifest-mac");
		for (const arch of ["aarch64", "x64"]) {
			const archive = join(mac, `MaxHQ_${v}_${arch}.app.tar.gz`);
			writeFileSync(archive, `archive ${arch}`);
			execFileSync(process.execPath, [here("sign.mjs"), archive], { env: keys });
			execFileSync(
				process.execPath,
				[here("sign.mjs"), "--next", "--out", `${archive}.next.sig`, archive],
				{ env: keys }
			);
		}
		const desktop = folder("manifest-desktop");
		const linux = desktopAssets().filter(({ target }) => target.includes("-linux-"));
		for (const { asset } of linux) writeFileSync(join(desktop, asset), `installer ${asset}`);
		assert.equal(sh("sign-desktop.sh", [desktop], { ...keys, MAXHQ_BRIDGE: "1" }).status, 0);
		const notes = join(dir, "manifest-notes.md");
		writeFileSync(notes, "What's new\n");

		const write = (name, env) => {
			const out = join(dir, `manifests-${name}`);
			const run = sh("write-manifests.sh", [notes, mac, desktop, out], {
				...bare(),
				VERSION: v,
				GITHUB_REPOSITORY: "owner/MaxHQ",
				CURRENT_PUBKEY: current.pub,
				NEXT_PUBKEY: next.pub,
				...env,
			});
			const read = (file) => JSON.parse(readFileSync(join(out, file), "utf8"));
			return { run, out, read };
		};
		const macs = ["darwin-aarch64", "darwin-x86_64"];
		const linuxKeys = linux.flatMap(({ platforms }) => platforms);
		const signer = (manifest) =>
			new Set(
				Object.values(manifest.platforms).map(({ signature }) =>
					keyIdText(signatureKeyId(signature))
				)
			);
		const keyOf = (key) => new Set([keyIdText(readPublicKey(key.pub).keyId)]);

		const old = write("old", { KEY_STATE: "old", UPDATE_MANIFEST: "latest.json" });
		assert.equal(old.run.status, 0, old.run.stderr);
		assert.deepEqual(old.run.stdout.trim().split("\n"), [join(old.out, "latest.json")]);
		assert.deepEqual(
			Object.keys(old.read("latest.json").platforms).sort(),
			[...macs, ...linuxKeys].sort()
		);
		assert.deepEqual(signer(old.read("latest.json")), keyOf(current));
		assert.equal(old.read("latest.json").notes, "What's new");

		const bridge = write("bridge", { KEY_STATE: "bridge", UPDATE_MANIFEST: "latest-v2.json" });
		assert.equal(bridge.run.status, 0, bridge.run.stderr);
		assert.deepEqual(bridge.run.stdout.trim().split("\n"), [
			join(bridge.out, "latest.json"),
			join(bridge.out, "latest-v2.json"),
		]);
		assert.deepEqual(Object.keys(bridge.read("latest.json").platforms), macs);
		assert.deepEqual(signer(bridge.read("latest.json")), keyOf(current));
		assert.deepEqual(
			Object.keys(bridge.read("latest-v2.json").platforms).sort(),
			[...macs, ...linuxKeys].sort()
		);
		assert.deepEqual(signer(bridge.read("latest-v2.json")), keyOf(next));
		assert.match(bridge.run.stderr, /"darwin-aarch64"/);

		// After the change: latest.json is the release before's, as it is.
		const frozen = join(dir, "manifest-frozen.json");
		writeFileSync(frozen, `${JSON.stringify({ version: "0.0.1", platforms: {} })}\n`);
		const after = { KEY_STATE: "new", UPDATE_MANIFEST: "latest-v2.json", FROZEN: frozen };
		const done = write("new", after);
		assert.equal(done.run.status, 0, done.run.stderr);
		assert.deepEqual(done.run.stdout.trim().split("\n"), [
			join(done.out, "latest-v2.json"),
			join(done.out, "latest.json"),
		]);
		assert.equal(
			readFileSync(join(done.out, "latest.json"), "utf8"),
			readFileSync(frozen, "utf8")
		);
		assert.deepEqual(signer(done.read("latest-v2.json")), keyOf(current));
		// Signed with the next key, every .sig must be by it.
		const wrong = write("wrong", { ...after, SIGN_KEY: "next" });
		assert.equal(wrong.run.status, 1);
		assert.match(wrong.run.stderr, /signature is by key .*, not /);
		writeFileSync(frozen, `${JSON.stringify({ version: v, platforms: {} })}\n`);
		const again = write("again", { ...after, PREVIOUS: "v0.0.1" });
		assert.equal(again.run.status, 1);
		assert.match(again.run.stderr, new RegExp(`latest.json on v0.0.1 already names ${v}`));
		const none = write("none", { UPDATE_MANIFEST: "latest.json" });
		assert.equal(none.run.status, 1);
		assert.match(none.run.stderr, /prepare-release gave no update key state/);
	}
);

test("android-sign leaves the build unsigned without a key and fails on half a key", () => {
	const build = folder("android");
	const out = join(dir, "android-out");
	const missing = mobile(["android-sign", build, out]);
	assert.equal(missing.status, 1);
	assert.match(missing.stderr, /has no app-release-unsigned.apk/);
	assert.equal(mobile(["android-sign", build]).status, 1, "both folders are needed");

	writeFileSync(join(build, "app-release-unsigned.apk"), "apk");
	writeFileSync(join(build, "app-release.aab"), "aab");
	const summary = join(dir, "summary.md");
	const none = mobile(
		["android-sign", build, out],
		bare({ GITHUB_ACTIONS: "true", GITHUB_STEP_SUMMARY: summary })
	);
	assert.equal(none.status, 0, none.stderr);
	assert.match(
		none.stdout,
		/::warning::There's no Android upload key, so the APK and the App Bundle stay unsigned/
	);
	assert.match(readFileSync(summary, "utf8"), /### Android build unsigned/);
	assert.ok(!existsSync(out), "nothing is written");

	const half = mobile(["android-sign", build, out], bare({ ANDROID_KEY_ALIAS: "upload" }));
	assert.equal(half.status, 1);
	assert.match(
		half.stderr,
		/ANDROID_KEYSTORE_BASE64, ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_PASSWORD isn't set/
	);
	assert.ok(!existsSync(out), "nothing is signed");
});

// validate-release.sh against the release make(base) gives, served from
// this process, so the script runs without blocking the server.
const haveCurl = spawnSync("curl", ["--version"]).status === 0;
const validate = async (make) => {
	let files = {};
	const server = createServer((req, res) => {
		const body = files[new URL(req.url, "http://localhost").pathname.slice(1)];
		res.writeHead(body === undefined ? 404 : 200);
		res.end(body);
	});
	await new Promise((done) => server.listen(0, "127.0.0.1", done));
	const base = `http://127.0.0.1:${server.address().port}`;
	files = make(base);
	try {
		return await new Promise((done) =>
			execFile(
				"sh",
				[here("validate-release.sh"), "owner/MaxHQ"],
				{ env: bare({ MAXHQ_RELEASE_BASE: base }) },
				(error, stdout) => done({ status: error ? error.code : 0, stdout })
			)
		);
	} finally {
		server.close();
	}
};

// A release with a Mac, a Linux installer, both Linux maxd, a Windows maxd
// and an Android build.
const release = (base, drop = []) => {
	const sum = (name) => `${createHash("sha256").update(name).digest("hex")}  ${name}`;
	const maxd = [
		"maxd-x86_64-unknown-linux-musl",
		"maxd-aarch64-unknown-linux-musl",
		maxdAsset("x86_64-pc-windows-msvc"),
	];
	const manifest = JSON.stringify({
		version: "0.2.0",
		platforms: {
			"darwin-aarch64": { signature: "c2ln", url: `${base}/MaxHQ_0.2.0_aarch64.app.tar.gz` },
			"linux-x86_64": { signature: "c2ln", url: `${base}/MaxHQ_x64.AppImage` },
			"linux-x86_64-appimage": { signature: "c2ln", url: `${base}/MaxHQ_x64.AppImage` },
		},
	});
	const files = {
		"latest.json": manifest,
		"latest-v2.json": manifest,
		"MaxHQ_0.2.0_aarch64.app.tar.gz": "app",
		"MaxHQ_0.2.0_aarch64.app.tar.gz.sig": "sig",
		"MaxHQ_x64.AppImage": "appimage",
		"MaxHQ_x64.AppImage.sig": "sig",
		"MaxHQ_aarch64.dmg": "dmg",
		"MaxHQ_x64.dmg": "dmg",
		SHA256SUMS: `${maxd.map(sum).join("\n")}\n`,
		"model-catalog.json": "{}",
		"model-catalog.json.sig": "sig",
		"mobile.json": JSON.stringify({ android: { build: 1, apk_url: `${base}/MaxHQ.apk` } }),
		"MaxHQ.apk": "apk",
		"MaxHQ.aab": "aab",
		...Object.fromEntries(maxd.map((name) => [name, "maxd"])),
	};
	for (const name of drop) delete files[name];
	return files;
};

test(
	"validate-release.sh checks the installers, each maxd and the APK",
	{ skip: !haveCurl },
	async () => {
		const good = await validate((base) => release(base));
		assert.equal(good.status, 0, good.stdout);
		assert.match(
			good.stdout,
			/--- latest-v2.json, linux-x86_64,linux-x86_64-appimage\n {2}download: OK\n {2}.sig: OK/
		);
		assert.match(good.stdout, /--- maxd\n(?: {2}.*: OK\n){3}/);
		assert.match(good.stdout, /maxd-x86_64-pc-windows-msvc: OK/);
		assert.match(good.stdout, /--- mobile.json, Android\n {2}MaxHQ.apk: OK\n {2}MaxHQ.aab: OK/);

		const broken = await validate((base) =>
			release(base, ["MaxHQ_x64.AppImage.sig", "maxd-x86_64-pc-windows-msvc", "MaxHQ.apk"])
		);
		assert.equal(broken.status, 1);
		assert.match(broken.stdout, /\.sig: HTTP 404 .*MaxHQ_x64\.AppImage\.sig/);
		assert.match(broken.stdout, /maxd-x86_64-pc-windows-msvc: HTTP 404/);
		assert.match(broken.stdout, /MaxHQ.apk: HTTP 404/);

		const noLinux = await validate((base) => {
			const files = release(base);
			files.SHA256SUMS = files.SHA256SUMS.split("\n").slice(1).join("\n");
			delete files["mobile.json"];
			return files;
		});
		assert.equal(noLinux.status, 1);
		assert.match(noLinux.stdout, /SHA256SUMS: has no maxd-x86_64-unknown-linux-musl/);
		assert.match(noLinux.stdout, /--- mobile.json: none/);
	}
);
