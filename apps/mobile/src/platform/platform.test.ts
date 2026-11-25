// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { t } from "@maxhq/shared/i18n";

import { font } from "../theme";
import { OS, platform, rulesFor } from ".";

describe("the platform rules", () => {
	it("keeps iOS as it was", () => {
		expect(rulesFor("ios")).toEqual({
			monoFont: "Menlo",
			sheetsBelowStatusBar: true,
			savesByShare: true,
			context: undefined,
			phoneName: "iPhone",
			pushService: "apns",
			answersInBackground: true,
			notificationChannels: false,
		});
	});

	it("gives Android its own font, full-screen sheets, folder saves, words and pushes", () => {
		expect(rulesFor("android")).toEqual({
			monoFont: "monospace",
			sheetsBelowStatusBar: false,
			savesByShare: false,
			context: "android",
			phoneName: "Android",
			pushService: "fcm",
			answersInBackground: false,
			notificationChannels: true,
		});
	});

	it("is iOS's outside Metro's Android build", () => {
		expect(OS).toBe("ios");
		expect(platform).toEqual(rulesFor("ios"));
		expect(font.mono).toBe("Menlo");
	});
});

describe("strings that name the system", () => {
	const ios = rulesFor("ios").context;
	const android = rulesFor("android").context;

	it("keep the iOS text without a context", () => {
		expect(t("Turn them on in iOS Settings.", { context: ios })).toBe(
			"Turn them on in iOS Settings."
		);
		expect(t("Update in TestFlight", { context: ios })).toBe("Update in TestFlight");
	});

	it("take the Android text with Android's context", () => {
		expect(t("Turn them on in iOS Settings.", { context: android })).toBe(
			"Turn them on in Android settings."
		);
		expect(t("iOS asks once.", { context: android })).toBe("Android asks you first.");
		expect(t("Update in TestFlight", { context: android })).toBe("Download update");
		expect(t("Open TestFlight", { context: android })).toBe("Download update");
		expect(t("Couldn't open TestFlight.", { context: android })).toBe(
			"Couldn't open the download link."
		);
	});

	it("fall back to the plain text where Android has no variant", () => {
		expect(t("Open Settings", { context: android })).toBe("Open Settings");
	});
});
