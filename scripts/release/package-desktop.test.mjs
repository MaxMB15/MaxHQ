// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
	buildEnv,
	bundleOutputs,
	endpoint,
	holds,
	sidecarName,
	tauriConfig,
} from "./package-desktop.mjs";

const repo = { MAXHQ_RELEASES_REPO: "owner/MaxHQ" };

test("the build config carries maxd and writes no updater artifacts", () => {
	const { config, warning } = tauriConfig({});
	assert.deepEqual(config, {
		bundle: { createUpdaterArtifacts: false, externalBin: ["binaries/maxd"] },
	});
	assert.match(warning, /can't look for updates/);
});

test("the build config names the release's manifest and key, as the Mac's does", () => {
	assert.deepEqual(tauriConfig(repo).config.plugins, {
		updater: {
			endpoints: ["https://github.com/owner/MaxHQ/releases/latest/download/latest.json"],
		},
	});
	const change = tauriConfig({
		...repo,
		MAXHQ_UPDATE_MANIFEST: "latest-v2.json",
		MAXHQ_UPDATE_PUBKEY: "dW50cnVzdGVk+/=",
	});
	assert.deepEqual(change.config.plugins.updater, {
		endpoints: [endpoint("owner/MaxHQ", "latest-v2.json")],
		pubkey: "dW50cnVzdGVk+/=",
	});
	assert.equal(change.warning, undefined);
	assert.equal(
		tauriConfig({ ...repo, MAXHQ_UPDATE_MANIFEST: "" }).config.plugins.updater.endpoints[0],
		endpoint("owner/MaxHQ", "latest.json")
	);
});

test("the build config refuses what package-macos.sh refuses", () => {
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

test("the sidecar is named for the target, with .exe on Windows", () => {
	assert.equal(sidecarName("x86_64-pc-windows-msvc"), "maxd-x86_64-pc-windows-msvc.exe");
	assert.equal(sidecarName("aarch64-unknown-linux-gnu"), "maxd-aarch64-unknown-linux-gnu");
});

test("the build sees no update key or Apple secret", () => {
	const env = buildEnv(
		{
			PATH: "/bin",
			TAURI_SIGNING_PRIVATE_KEY: "k",
			TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "p",
			TAURI_SIGNING_PRIVATE_KEY_NEXT: "n",
			TAURI_SIGNING_PRIVATE_KEY_PATH: "/k",
			APPLE_API_KEY: "a",
			MAXHQ_RELEASES_REPO: "owner/MaxHQ",
		},
		"windows"
	);
	assert.deepEqual(env, { PATH: "/bin", MAXHQ_RELEASES_REPO: "owner/MaxHQ" });
	assert.equal(buildEnv({}, "linux").APPIMAGE_EXTRACT_AND_RUN, "1");
});

test("each bundle type has exactly one file in its folder", () => {
	const dir = mkdtempSync(join(tmpdir(), "maxhq-bundles-"));
	try {
		const put = (path) => {
			mkdirSync(join(dir, path, ".."), { recursive: true });
			writeFileSync(join(dir, path), "x");
		};
		put("nsis/MaxHQ_0.2.0_x64-setup.exe");
		put("nsis/MaxHQ_0.2.0_x64-setup.nsi");
		put("msi/MaxHQ_0.2.0_x64_en-US.msi");
		put("appimage/MaxHQ_0.2.0_amd64.AppImage");
		mkdirSync(join(dir, "appimage/MaxHQ.AppDir"));
		put("deb/MaxHQ_0.2.0_amd64.deb");
		put("rpm/MaxHQ-0.2.0-1.x86_64.rpm");
		put("rpm/MaxHQ-0.2.1-1.x86_64.rpm");
		assert.deepEqual(bundleOutputs(dir, ["nsis", "msi"]), {
			nsis: join(dir, "nsis/MaxHQ_0.2.0_x64-setup.exe"),
			msi: join(dir, "msi/MaxHQ_0.2.0_x64_en-US.msi"),
		});
		assert.deepEqual(bundleOutputs(dir, ["appimage", "deb"]), {
			appimage: join(dir, "appimage/MaxHQ_0.2.0_amd64.AppImage"),
			deb: join(dir, "deb/MaxHQ_0.2.0_amd64.deb"),
		});
		assert.throws(() => bundleOutputs(dir, ["rpm"]), /has 2 \*\.rpm, not one/);
		rmSync(join(dir, "msi"), { recursive: true });
		assert.throws(() => bundleOutputs(dir, ["nsis", "msi"]), /has no \*\.msi, not one/);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("the smoke check finds text in a program's bytes", () => {
	const dir = mkdtempSync(join(tmpdir(), "maxhq-holds-"));
	try {
		const program = join(dir, "maxhq-desktop");
		const url = endpoint("owner/MaxHQ", "latest-v2.json");
		writeFileSync(
			program,
			Buffer.concat([Buffer.from([0, 1, 2]), Buffer.from(url), Buffer.from([0])])
		);
		assert.ok(holds(program, url));
		assert.ok(!holds(program, endpoint("owner/MaxHQ", "latest.json")));
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("the script refuses a target it doesn't build and a missing maxd", () => {
	const script = fileURLToPath(new URL("./package-desktop.mjs", import.meta.url));
	const run = (...args) =>
		spawnSync(process.execPath, [script, ...args], {
			encoding: "utf8",
			env: { ...process.env, GITHUB_ACTIONS: "" },
		});
	const musl = run("--target", "x86_64-unknown-linux-musl", "--maxd", script, "--out", "out");
	assert.equal(musl.status, 1);
	assert.match(musl.stderr, /isn't a desktop target/);
	const missing = run(
		"--target",
		"x86_64-unknown-linux-gnu",
		"--maxd",
		"/nonexistent/maxd",
		"--out",
		"o"
	);
	assert.equal(missing.status, 1);
	assert.match(missing.stderr, /there's no maxd at \/nonexistent\/maxd/);
	assert.match(run().stderr, /--target, --maxd and --out are required/);
});
