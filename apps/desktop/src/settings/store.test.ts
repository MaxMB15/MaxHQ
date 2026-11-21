// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Services } from "@/app/services";
import { shortcutListOf } from "@/shortcut-list/store";

import {
	back,
	escapeSettings,
	keepScroll,
	openSettings,
	scrollOf,
	settingsOf,
	showDeviceCard,
	showPlace,
} from "./store";

// The stores only key on the window's services.
const services = () => ({}) as unknown as Services;

describe("a place in Settings", () => {
	it("scrolls the page to a group or a card", () => {
		const window = services();
		openSettings(window, { kind: "page", at: "permissions" });
		expect(settingsOf(window).getState()).toMatchObject({
			open: true,
			detail: null,
			reveal: "settings-group-permissions",
		});
		showPlace(window, { kind: "page", at: "badge" });
		expect(settingsOf(window).getState().reveal).toBe("settings-card-badge");
	});

	it("shows a device's tab at one of its cards, or at the group", () => {
		const window = services();
		showDeviceCard(window, "dev-devbox", "terminals");
		expect(settingsOf(window).getState()).toMatchObject({
			open: true,
			tab: "dev-devbox",
			reveal: "device-card-terminals",
		});
		showDeviceCard(window, "dev-devbox", null);
		expect(settingsOf(window).getState().reveal).toBe("settings-group-devices");
	});

	it("opens the keyboard shortcuts in place of Settings", () => {
		const window = services();
		openSettings(window, { kind: "page", at: null });
		openSettings(window, { kind: "shortcuts" });
		expect(settingsOf(window).getState().open).toBe(false);
		expect(shortcutListOf(window).getState().open).toBe(true);
	});
});

describe("a view of its own", () => {
	it("shows in place of the page, and Back leaves it where the page was", () => {
		const window = services();
		openSettings(window, { kind: "page", at: null });
		keepScroll(window, 640);
		showPlace(window, { kind: "scheduled-tasks" });
		expect(settingsOf(window).getState().detail).toEqual({ kind: "scheduled-tasks" });
		back(window);
		expect(settingsOf(window).getState()).toMatchObject({ open: true, detail: null });
		expect(scrollOf(window)).toBe(640);
	});

	it("goes back on the first Esc and closes on the second", () => {
		const window = services();
		openSettings(window, { kind: "skills" });
		expect(escapeSettings(window)).toBe(true);
		expect(settingsOf(window).getState()).toMatchObject({ open: true, detail: null });
		expect(escapeSettings(window)).toBe(false);
		expect(settingsOf(window).getState().open).toBe(false);
	});

	it("opens a new place at its top, and the same window where you left it", () => {
		const window = services();
		openSettings(window, { kind: "page", at: null });
		keepScroll(window, 300);
		openSettings(window);
		expect(scrollOf(window)).toBe(300);
		escapeSettings(window);
		openSettings(window, { kind: "page", at: "accounts" });
		expect(scrollOf(window)).toBe(0);
	});
});
