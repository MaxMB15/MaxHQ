// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
	iosConfig,
	iosLocalFromEnv,
	MISSING_LOCAL,
	parseIosLocal,
	readIosLocal,
	resolveVariant,
} from "./ios-config.ts";

const local = { teamId: "ABCDE12345", bundleId: "com.example.maxhq" };
const infoPlist = {
	NSLocalNetworkUsageDescription: "MaxHQ reaches your Mac and other devices on your network.",
	NSAppTransportSecurity: { NSAllowsLocalNetworking: true },
};

describe("parseIosLocal", () => {
	it("reads a filled-in file", () => {
		expect(parseIosLocal(JSON.stringify(local))).toEqual(local);
	});

	it("names the file and the field without echoing what's in it", () => {
		const bad = (text: string) => () => parseIosLocal(text);
		expect(bad("{")).toThrow("apps/mobile/ios.local.json isn't valid JSON");
		expect(bad('{"teamId":"SECRETISH","bundleId":"com.example.maxhq"}')).toThrow(
			/^apps\/mobile\/ios\.local\.json needs "teamId"/
		);
		expect(bad('{"teamId":"ABCDE12345","bundleId":"nodots"}')).toThrow(/needs "bundleId"/);
		expect(bad("null")).toThrow(/needs "teamId"/);
		try {
			parseIosLocal('{"teamId":"SECRETISH","bundleId":"x"}');
		} catch (error) {
			expect(String(error)).not.toContain("SECRETISH");
		}
	});

	it("turns down the tracked example until it's filled in", () => {
		const example = readFileSync(
			join(import.meta.dirname, "../ios.local.example.json"),
			"utf8"
		);
		expect(JSON.parse(example)).toEqual({ teamId: "<TEAM_ID>", bundleId: "com.example.maxhq" });
		expect(() => parseIosLocal(example)).toThrow(/needs "teamId"/);
	});
});

describe("iosLocalFromEnv", () => {
	it("writes the release workflows' IDs as the file has them", () => {
		const text = iosLocalFromEnv({
			IOS_TEAM_ID: local.teamId,
			IOS_BUNDLE_ID: local.bundleId,
			OTHER: "left out",
		});
		expect(text).toBe(`${JSON.stringify(local, null, "\t")}\n`);
		expect(parseIosLocal(text)).toEqual(local);
	});

	it("turns down a missing or bad ID", () => {
		expect(() => iosLocalFromEnv({ IOS_BUNDLE_ID: local.bundleId })).toThrow(/needs "teamId"/);
		expect(() =>
			iosLocalFromEnv({ IOS_TEAM_ID: local.teamId, IOS_BUNDLE_ID: "nodots" })
		).toThrow(/needs "bundleId"/);
	});
});

describe("readIosLocal", () => {
	it("is null when the file isn't there and reads it when it is", () => {
		const dir = mkdtempSync(join(tmpdir(), "maxhq-ios-"));
		try {
			expect(readIosLocal(dir)).toBeNull();
			writeFileSync(join(dir, "ios.local.json"), JSON.stringify(local));
			expect(readIosLocal(dir)).toEqual(local);
		} finally {
			rmSync(dir, { recursive: true });
		}
	});
});

describe("resolveVariant", () => {
	it("is production unless development is asked for", () => {
		expect(resolveVariant("development")).toBe("development");
		expect(resolveVariant("production")).toBe("production");
		expect(resolveVariant("preview")).toBe("production");
		expect(resolveVariant(undefined)).toBe("production");
	});
});

describe("iosConfig", () => {
	it("fails a device build in one sentence naming the file", () => {
		expect(() => iosConfig("development", null, true)).toThrow(MISSING_LOCAL);
		expect(MISSING_LOCAL).toBe(
			"Add apps/mobile/ios.local.json with your Team ID and bundle ID before building for a device."
		);
	});

	it("falls back to the example ID with no signing, push or app group on the simulator", () => {
		expect(iosConfig("production", null, false)).toEqual({
			name: "MaxHQ",
			scheme: "maxhq",
			bundleIdentifier: "com.example.maxhq",
			entitlements: {},
			infoPlist,
			push: false,
			appGroup: null,
		});
		expect(iosConfig("development", null, false).bundleIdentifier).toBe(
			"com.example.maxhq.dev"
		);
	});

	it("gives each variant its own bundle ID, scheme and groups so both fit on one phone", () => {
		const production = iosConfig("production", local, true);
		const development = iosConfig("development", local, true);
		expect(production).toEqual({
			name: "MaxHQ",
			scheme: "maxhq",
			bundleIdentifier: "com.example.maxhq",
			appleTeamId: "ABCDE12345",
			entitlements: {
				"aps-environment": "production",
				"com.apple.security.application-groups": ["group.com.example.maxhq"],
				"keychain-access-groups": ["$(AppIdentifierPrefix)com.example.maxhq"],
				"com.apple.developer.usernotifications.time-sensitive": true,
			},
			infoPlist,
			push: true,
			appGroup: "group.com.example.maxhq",
		});
		expect(development.name).toBe("MaxHQ Dev");
		expect(development.scheme).toBe("maxhq-dev");
		expect(development.bundleIdentifier).toBe("com.example.maxhq.dev");
		expect(development.entitlements["aps-environment"]).toBe("development");
		expect(development.appGroup).toBe("group.com.example.maxhq.dev");
	});

	it("lets plain http through to local addresses only, signed or not", () => {
		for (const config of [
			iosConfig("production", null, false),
			iosConfig("development", local, true),
			iosConfig("production", local, true),
		]) {
			expect(config.infoPlist.NSAppTransportSecurity).toEqual({
				NSAllowsLocalNetworking: true,
			});
		}
	});

	it("uses the local file on the simulator too when it's there", () => {
		expect(iosConfig("production", local, false).appleTeamId).toBe("ABCDE12345");
	});
});
