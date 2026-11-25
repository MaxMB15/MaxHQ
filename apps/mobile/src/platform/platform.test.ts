// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";

import { setInterfaceOs, t } from "@maxhq/shared/i18n";

import { font } from "../theme";
import { OS, platform, rulesFor } from ".";

describe("the platform rules", () => {
	it("keeps iOS as it was", () => {
		expect(rulesFor("ios")).toEqual({
			monoFont: "Menlo",
			sheetsBelowStatusBar: true,
			savesByShare: true,
			opensPdf: true,
			updatesThrough: "testflight",
			phoneName: "iPhone",
			pushService: "apns",
			answersInBackground: true,
			notificationChannels: false,
		});
	});

	it("gives Android its own font, full-screen sheets, folder saves, viewer, updates and pushes", () => {
		expect(rulesFor("android")).toEqual({
			monoFont: "monospace",
			sheetsBelowStatusBar: false,
			savesByShare: false,
			opensPdf: false,
			updatesThrough: "apk",
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
	afterEach(() => setInterfaceOs(null));

	it("keep the iOS text on an iPhone", () => {
		setInterfaceOs("ios");
		expect(t("Turn them on in iOS Settings.")).toBe("Turn them on in iOS Settings.");
		expect(t("Update in TestFlight")).toBe("Update in TestFlight");
	});

	it("take the Android text on an Android phone", () => {
		setInterfaceOs("android");
		expect(t("Turn them on in iOS Settings.")).toBe("Turn them on in Android settings.");
		expect(t("iOS asks once.")).toBe("Android asks you first.");
		expect(t("Update in TestFlight")).toBe("Download update");
		expect(t("Open TestFlight")).toBe("Download update");
		expect(t("Couldn't open TestFlight.")).toBe("Couldn't open the download link.");
	});

	it("fall back to the plain text where Android has no variant", () => {
		setInterfaceOs("android");
		expect(t("Open Settings")).toBe("Open Settings");
	});
});
