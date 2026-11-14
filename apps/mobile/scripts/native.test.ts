// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { resolve } from "node:path";

import { createFingerprintAsync } from "expo/fingerprint";
import { describe, expect, it } from "vitest";

import {
	checkNativeBuild,
	coreSteps,
	devBuildSteps,
	exportOptions,
	installSteps,
	needsDevBuild,
	NO_PHONE,
	onlyPhone,
	RUST_TARGETS,
	testflightSteps,
	type NativeBuild,
} from "./native.ts";

const local = { teamId: "ABCDE12345", bundleId: "com.example.maxhq" };
const line = (step: { cmd: string; args: string[] }) => [step.cmd, ...step.args].join(" ");

describe("coreSteps", () => {
	it("builds the mobile crate once per Apple target, then the bindings", () => {
		expect(RUST_TARGETS).toEqual(["aarch64-apple-ios", "aarch64-apple-ios-sim"]);
		const debug = coreSteps(false);
		expect(debug.map(line)).toEqual([
			...RUST_TARGETS.map((t) => `cargo build -p maxhq-mobile --target ${t}`),
			"ubrn build ios --config ubrn.config.yaml --no-cargo --and-generate",
		]);
		expect(debug.slice(0, -1).every((step) => step.cwd === "repo" && !step.env)).toBe(true);
		expect(debug.at(-1)!.cwd).toBe("core");
	});

	it("uses the release profile with thin LTO for TestFlight", () => {
		const release = coreSteps(true);
		for (const step of release) expect(step.args.at(-1)).toBe("--release");
		for (const step of release.slice(0, -1)) {
			expect(step.env).toEqual({ CARGO_PROFILE_RELEASE_LTO: "thin" });
		}
	});
});

describe("devBuildSteps", () => {
	it("installs a phone's build with devicectl on its own, and leaves Metro for later", () => {
		const { build, install } = devBuildSteps(true, "00008110-000A");
		expect(line(build[0]!)).toBe("expo prebuild --clean --platform ios");
		expect(line(install!)).toBe(
			"xcrun devicectl device install app --device 00008110-000A .expo/xcode/Build/Products/Debug-iphoneos/MaxHQDev.app"
		);
		expect(install!.cwd).toBe("app");
		expect([...build, install!].some((step) => step.args.includes("run:ios"))).toBe(false);
	});

	it("stops before building for a phone when none was found", () => {
		expect(() => devBuildSteps(true)).toThrow(NO_PHONE);
	});

	it("builds for a phone with provisioning updates, so push and the app group get a profile", () => {
		const [, build] = devBuildSteps(true, "00008110-000A").build;
		expect(build!.cmd).toBe("xcodebuild");
		expect(build!.args).toEqual(
			expect.arrayContaining([
				"ios/MaxHQDev.xcworkspace",
				"-allowProvisioningUpdates",
				"-allowProvisioningDeviceRegistration",
				"build",
			])
		);
		expect(build!.env).toMatchObject({ APP_VARIANT: "development", MAXHQ_IOS_DEVICE: "1" });
	});

	it("lets the simulator build fall back", () => {
		const { build, install } = devBuildSteps(false);
		expect(line(build[1]!)).toBe("expo run:ios --no-bundler");
		expect(build[1]!.env).not.toHaveProperty("MAXHQ_IOS_DEVICE");
		expect(install).toBeUndefined();
	});
});

describe("onlyPhone", () => {
	const phone = (udid: string, pairingState = "paired", reality = "physical") => ({
		hardwareProperties: { platform: "iOS", reality, udid },
		connectionProperties: { pairingState },
	});

	it("names the one paired iPhone and nothing when it isn't sure", () => {
		expect(
			onlyPhone([phone("a"), phone("b", "unpaired"), phone("c", "paired", "virtual")])
		).toBe("a");
		expect(onlyPhone([phone("a"), phone("b")])).toBeUndefined();
		expect(onlyPhone([])).toBeUndefined();
	});
});

describe("needsDevBuild", () => {
	it("rebuilds only when this destination's fingerprint changed", () => {
		expect(needsDevBuild({}, true, "a")).toBe(true);
		expect(needsDevBuild({ device: "a" }, true, "a")).toBe(false);
		expect(needsDevBuild({ device: "a" }, true, "b")).toBe(true);
		expect(needsDevBuild({ device: "a" }, false, "a")).toBe(true);
	});
});

describe("testflightSteps", () => {
	it("prebuilds production from clean, archives and uploads with automatic signing", () => {
		const [prebuild, archive, upload] = testflightSteps(local);
		expect(line(prebuild!)).toBe("expo prebuild --clean --platform ios");
		expect(prebuild!.env).toMatchObject({ APP_VARIANT: "production", MAXHQ_IOS_DEVICE: "1" });
		expect(line(archive!)).toContain(
			"-workspace ios/MaxHQ.xcworkspace -scheme MaxHQ -configuration Release"
		);
		expect(archive!.args).toContain("DEVELOPMENT_TEAM=ABCDE12345");
		expect(archive!.args.at(-1)).toBe("archive");
		expect(upload!.args).toEqual(
			expect.arrayContaining(["-exportArchive", "-allowProvisioningUpdates"])
		);
		const plist = exportOptions(local.teamId, "testflight");
		expect(plist).toContain("<key>method</key>\n\t<string>app-store-connect</string>");
		expect(plist).toContain("<key>destination</key>\n\t<string>upload</string>");
		expect(plist).toContain("<key>manageAppVersionAndBuildNumber</key>\n\t<false/>");
		expect(plist).toContain("<key>teamID</key>\n\t<string>ABCDE12345</string>");
	});
});

describe("installSteps", () => {
	it("archives the production variant in Release the way TestFlight does", () => {
		const steps = installSteps(local, "00008110-000A");
		expect(steps.slice(0, 3)).toEqual(testflightSteps(local));
		expect(steps[0]!.env).toMatchObject({ APP_VARIANT: "production", MAXHQ_IOS_DEVICE: "1" });
		expect(line(steps[1]!)).toContain("-scheme MaxHQ -configuration Release");
	});

	it("installs the exported app on the phone with devicectl", () => {
		const [, , , unpack, install, ...rest] = installSteps(local, "00008110-000A");
		expect(line(unpack!)).toBe("ditto -x -k ios/build/export/MaxHQ.ipa ios/build/export");
		expect(line(install!)).toBe(
			"xcrun devicectl device install app --device 00008110-000A ios/build/export/Payload/MaxHQ.app"
		);
		expect(rest).toEqual([]);
	});

	it("exports ad hoc with automatic signing", () => {
		const plist = exportOptions(local.teamId, "install");
		expect(plist).toContain("<key>method</key>\n\t<string>release-testing</string>");
		expect(plist).toContain("<key>destination</key>\n\t<string>export</string>");
		expect(plist).toContain("<key>signingStyle</key>\n\t<string>automatic</string>");
		expect(plist).toContain("<key>teamID</key>\n\t<string>ABCDE12345</string>");
		expect(plist).not.toContain("manageAppVersionAndBuildNumber");
	});

	it("stops before building when no phone was found", () => {
		expect(() => installSteps(local, undefined)).toThrow(NO_PHONE);
	});
});

describe("checkNativeBuild", () => {
	const uploaded = (buildNumber: number): NativeBuild => ({
		buildNumber,
		lastUpload: { buildNumber: 3, fingerprint: "old" },
	});

	it("lets the first build through", () => {
		expect(checkNativeBuild({ buildNumber: 1, lastUpload: null }, "any", true)).toBeNull();
	});

	it("refuses a changed fingerprint under the same build number", () => {
		expect(checkNativeBuild(uploaded(3), "new", false)).toBe(
			"The native fingerprint changed since build 3 went to TestFlight; raise buildNumber in apps/mobile/native-build.json."
		);
	});

	it("lets a changed fingerprint through once the number is raised", () => {
		expect(checkNativeBuild(uploaded(4), "new", false)).toBeNull();
		expect(checkNativeBuild(uploaded(4), "new", true)).toBeNull();
	});

	it("passes a JavaScript-only change in CI but not as a second upload of the same number", () => {
		expect(checkNativeBuild(uploaded(3), "old", false)).toBeNull();
		expect(checkNativeBuild(uploaded(3), "old", true)).toMatch(
			/^Build 3 is already on TestFlight/
		);
	});

	it("refuses a number below the last upload", () => {
		expect(checkNativeBuild(uploaded(2), "old", false)).toMatch(/^Build 2 is below build 3/);
	});
});

describe("fingerprint.config.js", () => {
	// It hashes the Rust core twice, which alone takes about 14 s and can pass
	// 30 s while the other suites run.
	it("counts the Rust core and the icon as native and leaves the variant out", async () => {
		const appDir = resolve(import.meta.dirname, "..");
		const before = process.env.APP_VARIANT;
		try {
			process.env.APP_VARIANT = "production";
			const production = await createFingerprintAsync(appDir, { silent: true });
			process.env.APP_VARIANT = "development";
			const development = await createFingerprintAsync(appDir, { silent: true });
			expect(development.hash).toBe(production.hash);
			const paths = production.sources.map((s) => ("filePath" in s ? s.filePath : s.id));
			expect(paths).toEqual(
				expect.arrayContaining([
					"../../crates",
					"../../Cargo.toml",
					"../../Cargo.lock",
					"assets/icon.png",
				])
			);
			expect(
				production.sources.some((s) => s.type === "contents" && s.id === "expoConfig")
			).toBe(false);
		} finally {
			if (before === undefined) delete process.env.APP_VARIANT;
			else process.env.APP_VARIANT = before;
		}
	}, 120_000);
});
