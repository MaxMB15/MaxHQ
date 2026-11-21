// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { createFingerprintAsync } from "expo/fingerprint";
import { describe, expect, it } from "vitest";

import {
	AD_HOC_SIGNING,
	API_KEY_ENV,
	apiKeyFromEnv,
	appInIpa,
	archivedApp,
	archivePath,
	checkNativeBuild,
	coreSteps,
	devBuildSteps,
	exportOptions,
	exportSteps,
	installSteps,
	ipaPath,
	keylessArchiveSteps,
	missingEntitlements,
	needsDevBuild,
	NO_PHONE,
	onlyPhone,
	PIPELINE_BUILD_FLOOR,
	pipelineBuildNumber,
	resolveBuildNumber,
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

	it("signs and uploads with an API key when there is one, and hides its IDs", () => {
		const key = { path: "/tmp/AuthKey.p8", id: "KEY1234567", issuer: "issuer-uuid" };
		const [prebuild, archive, upload] = testflightSteps(local, key);
		expect(prebuild!.hide).toBeUndefined();
		const auth = [
			"-authenticationKeyPath",
			"/tmp/AuthKey.p8",
			"-authenticationKeyID",
			"KEY1234567",
			"-authenticationKeyIssuerID",
			"issuer-uuid",
		];
		for (const step of [archive!, upload!]) {
			expect(line(step)).toContain(`-allowProvisioningUpdates ${auth.join(" ")}`);
			expect(step.hide).toEqual(["KEY1234567", "issuer-uuid"]);
		}
		expect(archive!.args.at(-1)).toBe("archive");
		expect(testflightSteps(local)[1]!.args).not.toContain("-authenticationKeyPath");
	});

	it("keeps the same build as an .ipa for the release workflow to upload later", () => {
		const plist = exportOptions(local.teamId, "ipa");
		expect(plist).toContain("<key>method</key>\n\t<string>app-store-connect</string>");
		expect(plist).toContain("<key>destination</key>\n\t<string>export</string>");
		expect(plist).toContain("<key>manageAppVersionAndBuildNumber</key>\n\t<false/>");
		expect(plist).toContain("<key>signingStyle</key>\n\t<string>automatic</string>");
		expect(plist).toContain("<key>teamID</key>\n\t<string>ABCDE12345</string>");
		// .github/workflows/release.yml keeps the artifact from this path.
		expect(ipaPath(local)).toBe("ios/build/export/MaxHQ.ipa");
	});
});

describe("the release workflow's archive and export", () => {
	const key = { path: "/tmp/AuthKey.p8", id: "KEY1234567", issuer: "issuer-uuid" };

	it("archives the same app as TestFlight, signed ad hoc with no key or account", () => {
		const [prebuild, archive, ...rest] = keylessArchiveSteps(local);
		expect(rest).toEqual([]);
		expect(prebuild).toEqual(testflightSteps(local)[0]);
		expect(line(archive!)).toContain(
			"-workspace ios/MaxHQ.xcworkspace -scheme MaxHQ -configuration Release"
		);
		expect(archive!.args).toEqual(expect.arrayContaining(AD_HOC_SIGNING));
		expect(archive!.args).toContain("DEVELOPMENT_TEAM=ABCDE12345");
		expect(archive!.args).not.toContain("-allowProvisioningUpdates");
		expect(archive!.args.join(" ")).not.toMatch(/-authentication/);
		expect(archive!.args.at(-1)).toBe("archive");
		expect(archivePath(local)).toBe("ios/build/MaxHQ.xcarchive");
		expect(line(archive!)).toContain(`-archivePath ${archivePath(local)}`);
		expect(archivedApp(local)).toBe(
			"ios/build/MaxHQ.xcarchive/Products/Applications/MaxHQ.app"
		);
	});

	it("exports that archive as an .ipa with the key, and hides its IDs", () => {
		const [exported, ...rest] = exportSteps(local, key);
		expect(rest).toEqual([]);
		expect(exported).toEqual(testflightSteps(local, key)[2]);
		expect(line(exported!)).toContain(`-exportArchive -archivePath ${archivePath(local)}`);
		expect(line(exported!)).toContain(
			"-allowProvisioningUpdates -authenticationKeyPath /tmp/AuthKey.p8"
		);
		expect(exported!.hide).toEqual(["KEY1234567", "issuer-uuid"]);
		expect(appInIpa(local)).toBe("Payload/MaxHQ.app");
	});

	describe("missingEntitlements", () => {
		const signed = {
			"aps-environment": "production",
			"com.apple.security.application-groups": ["group.com.example.maxhq"],
			"keychain-access-groups": ["ABCDE12345.com.example.maxhq"],
			"com.apple.developer.usernotifications.time-sensitive": true,
		};

		it("passes an app that carries push, the app group, the Keychain group and time-sensitive prompts", () => {
			expect(missingEntitlements(local, signed, true)).toEqual([]);
			expect(missingEntitlements(local, signed, false)).toEqual([]);
		});

		it("names each entitlement an app signed without them lost", () => {
			expect(missingEntitlements(local, {}, false)).toEqual([
				"aps-environment",
				"com.apple.security.application-groups",
				"keychain-access-groups",
				"com.apple.developer.usernotifications.time-sensitive",
			]);
			const otherGroup = {
				...signed,
				"com.apple.security.application-groups": ["group.com.example.other"],
			};
			expect(missingEntitlements(local, otherGroup, true)).toEqual([
				"com.apple.security.application-groups",
			]);
		});

		it("asks for production push only from the exported app", () => {
			const development = { ...signed, "aps-environment": "development" };
			expect(missingEntitlements(local, development, false)).toEqual([]);
			expect(missingEntitlements(local, development, true)).toEqual(["aps-environment"]);
		});
	});

	// The upload job runs export with no packages installed, so mobile.ts and
	// what it loads at start name only Node's modules and each other.
	it("loads nothing at start but Node's modules and the scripts beside it", () => {
		const importsOf = (file: string) =>
			[
				...readFileSync(resolve(import.meta.dirname, file), "utf8").matchAll(
					/^import\s(?:[^;]*?from\s+)?"([^"]+)"/gms
				),
			].map((match) => match[1]);
		expect(importsOf("mobile.ts")).toEqual(
			expect.arrayContaining(["node:child_process", "./native.ts", "./ios-config.ts"])
		);
		for (const file of ["mobile.ts", "native.ts", "ios-config.ts", "release-config.ts"]) {
			for (const name of importsOf(file)) expect(name).toMatch(/^(node:|\.\/)/);
		}
	});
});

describe("apiKeyFromEnv", () => {
	const full = {
		[API_KEY_ENV.path]: "/tmp/AuthKey.p8",
		[API_KEY_ENV.id]: "KEY1234567",
		[API_KEY_ENV.issuer]: "issuer-uuid",
	};

	it("reads all three or none", () => {
		expect(apiKeyFromEnv({})).toBeNull();
		expect(apiKeyFromEnv(full)).toEqual({
			path: "/tmp/AuthKey.p8",
			id: "KEY1234567",
			issuer: "issuer-uuid",
		});
	});

	it("names what's missing from half a key and echoes nothing that's set", () => {
		const half = { ...full, [API_KEY_ENV.issuer]: "" };
		expect(() => apiKeyFromEnv(half)).toThrow(`${API_KEY_ENV.issuer} isn't set`);
		try {
			apiKeyFromEnv(half);
		} catch (error) {
			expect(String(error)).not.toContain("KEY1234567");
		}
	});
});

describe("build numbers", () => {
	it("numbers the workflow's builds by run and attempt, above every local one", () => {
		expect(pipelineBuildNumber(7, 1)).toBe(10701);
		expect(pipelineBuildNumber(7, 2)).toBe(10702);
		expect(pipelineBuildNumber(8, 1)).toBeGreaterThan(pipelineBuildNumber(7, 99));
		expect(pipelineBuildNumber(1, 1)).toBeGreaterThan(PIPELINE_BUILD_FLOOR);
	});

	it("refuses a run or attempt it can't number", () => {
		expect(() => pipelineBuildNumber(Number.NaN, 1)).toThrow(/run number/);
		expect(() => pipelineBuildNumber(0, 1)).toThrow(/run number/);
		expect(() => pipelineBuildNumber(7, 0)).toThrow(/attempt/);
		expect(() => pipelineBuildNumber(7, 100)).toThrow(/attempt/);
	});

	it("takes the workflow's number when set, else native-build.json's", () => {
		const record = { buildNumber: 3, lastUpload: null };
		expect(resolveBuildNumber(record, undefined)).toBe(3);
		expect(resolveBuildNumber(record, "")).toBe(3);
		expect(resolveBuildNumber(record, "10702")).toBe(10702);
		for (const bad of ["5", "10000", "10702.1", "0x2a00", "abc"]) {
			expect(() => resolveBuildNumber(record, bad)).toThrow(/MAXHQ_BUILD_NUMBER/);
		}
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

	it("keeps local numbers out of the release workflow's range", () => {
		expect(
			checkNativeBuild({ buildNumber: PIPELINE_BUILD_FLOOR, lastUpload: null }, "any", false)
		).toBe(
			"Build 10000 is in the release workflow's range; keep buildNumber in apps/mobile/native-build.json below 10000."
		);
		expect(
			checkNativeBuild(
				{ buildNumber: PIPELINE_BUILD_FLOOR - 1, lastUpload: null },
				"any",
				true
			)
		).toBeNull();
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
