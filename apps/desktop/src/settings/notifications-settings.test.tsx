// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { NotificationAccess, NotificationPrefs } from "@maxhq/shared/bindings";
import { setBadgePrefs } from "@/native/dock-badge";
import { DEFAULT_NOTIFICATION_PREFS } from "@/native/notifications";
import { searchWords } from "@maxhq/shared/lib/words";
import { DEFAULT_BADGE_PREFS } from "@maxhq/shared/view/badge";

import { DOCK_BADGE_TERMS, DockBadgeSection } from "./dock-badge-settings";
import { NotificationsSettingsPage } from "./notifications-settings";
import { findIn } from "./search";

const app = globalThis as { isTauri?: boolean };

/** What the app holds, and the calls the page made. */
let held: NotificationPrefs;
let access: NotificationAccess;
const calls: [string, InvokeArgs | undefined][] = [];

beforeEach(() => {
	app.isTauri = true;
	held = { ...DEFAULT_NOTIFICATION_PREFS, kinds: [...DEFAULT_NOTIFICATION_PREFS.kinds] };
	access = { permission: "allowed", badge: true };
	calls.length = 0;
	localStorage.clear();
	setBadgePrefs(DEFAULT_BADGE_PREFS);
	mockIPC((command, args) => {
		calls.push([command, args]);
		switch (command) {
			case "notification_prefs":
				return held;
			case "set_notification_prefs":
				held = (args as { prefs: NotificationPrefs }).prefs;
				return null;
			case "notification_permission":
				return access;
			case "send_test_notification":
			case "open_notification_settings":
				return null;
		}
	});
});

afterEach(() => {
	clearMocks();
	delete app.isTauri;
});

// The group's two cards, as the Settings page shows them.
const renderPage = async () => {
	render(
		<>
			<NotificationsSettingsPage />
			<DockBadgeSection />
		</>
	);
	// Loaded once the master switch takes clicks.
	await waitFor(() =>
		expect(screen.getByRole("switch", { name: "Show notifications" })).not.toHaveAttribute(
			"data-disabled"
		)
	);
};

const toggle = (name: string) =>
	act(async () => {
		fireEvent.click(screen.getByRole("switch", { name }));
		await Promise.resolve();
	});

const checked = (name: string) =>
	screen.getByRole("switch", { name }).getAttribute("aria-checked") === "true";

const sent = (command: string) => calls.filter(([name]) => name === command);

/** A card on the page, by its title. */
const card = (title: string) => within(screen.getByRole("region", { name: title }));

describe("the Notifications page", () => {
	it("starts with every kind on and a sound for prompts only", async () => {
		await renderPage();
		for (const name of ["Show notifications", "Needs input", "Error", "Finished", "Limit"]) {
			expect(checked(name), name).toBe(true);
		}
		expect(checked("Sound for prompts")).toBe(true);
		expect(checked("Sound for other alerts")).toBe(false);
		expect(screen.getByRole("status")).toHaveTextContent("macOS shows MaxHQ's notifications.");
	});

	it("saves each switch as it changes", async () => {
		await renderPage();
		await toggle("Finished");
		await waitFor(() => expect(held.kinds).toEqual(["needs_input", "error", "limit"]));
		await toggle("Sound for other alerts");
		await waitFor(() => expect(held.other_sound).toBe(true));
		expect(held.kinds).toEqual(["needs_input", "error", "limit"]);
	});

	it("turning notifications off greys out the rest", async () => {
		await renderPage();
		await toggle("Show notifications");
		await waitFor(() => expect(held.show).toBe(false));
		for (const name of ["Needs input", "Limit", "Sound for prompts"]) {
			expect(screen.getByRole("switch", { name })).toHaveAttribute("data-disabled");
		}
		// The kinds stay as they were, for when it comes back on.
		expect(held.kinds).toHaveLength(4);
	});

	it("sends a test notification", async () => {
		await renderPage();
		fireEvent.click(screen.getByRole("button", { name: "Send a test notification" }));
		await waitFor(() => expect(sent("send_test_notification")).toHaveLength(1));
	});

	it("opens System Settings when macOS says no", async () => {
		access = { permission: "denied", badge: false };
		await renderPage();
		expect(await screen.findByRole("status")).toHaveTextContent(
			"turned off for MaxHQ in System Settings"
		);
		fireEvent.click(
			card("Notifications").getByRole("button", { name: "Open System Settings" })
		);
		await waitFor(() => expect(sent("open_notification_settings")).toHaveLength(1));
	});

	it("reads macOS again when the window comes back", async () => {
		access = { permission: "not_decided", badge: false };
		await renderPage();
		expect(await screen.findByRole("status")).toHaveTextContent("hasn't asked");
		access = { permission: "allowed", badge: true };
		act(() => {
			window.dispatchEvent(new Event("focus"));
		});
		expect(await screen.findByText("macOS shows MaxHQ's notifications.")).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Open System Settings" })).toBeNull();
	});

	it("says when the build can't show any, and can't send a test", async () => {
		access = { permission: "unavailable", badge: true };
		await renderPage();
		const status = await screen.findByText(/doesn't run from an app bundle/);
		expect(
			within(status.closest("section")!).getByRole("button", {
				name: "Send a test notification",
			})
		).toBeDisabled();
	});
});

describe("the badge card", () => {
	const HIDDEN = /macOS hides the (Dock badge|count in the Dock)/;
	// What macOS hides is a Mac's to say.
	beforeEach(() => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15"
		);
	});
	afterEach(() => {
		vi.restoreAllMocks();
	});

	it("leaves macOS's reasons out on Windows and Linux", async () => {
		access = { permission: "denied", badge: false };
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/131.0.0.0"
		);
		await renderPage();
		await screen.findByText("Notifications are turned off for MaxHQ in System Settings.");
		expect(card("Badge").queryByText(HIDDEN)).toBeNull();
		expect(card("Badge").queryByRole("button")).toBeNull();
	});

	it("says nothing more while macOS shows the badge", async () => {
		await renderPage();
		await screen.findByText("macOS shows MaxHQ's notifications.");
		expect(card("Badge").queryByText(HIDDEN)).toBeNull();
		expect(card("Badge").queryByRole("button")).toBeNull();
	});

	it("says when badges are off, and opens System Settings", async () => {
		access = { permission: "allowed", badge: false };
		await renderPage();
		expect(
			await card("Badge").findByText(
				"Badges are turned off for MaxHQ in System Settings, so macOS hides the count in the Dock."
			)
		).toBeInTheDocument();
		fireEvent.click(card("Badge").getByRole("button", { name: "Open System Settings" }));
		await waitFor(() => expect(sent("open_notification_settings")).toHaveLength(1));
	});

	it.each(["denied", "not_decided"] as const)(
		"says the badge waits on notifications when they're %s",
		async (permission) => {
			access = { permission, badge: false };
			await renderPage();
			expect(
				await card("Badge").findByText(
					"macOS hides the Dock badge until MaxHQ's notifications are allowed."
				)
			).toBeInTheDocument();
			fireEvent.click(card("Badge").getByRole("button", { name: "Open System Settings" }));
			await waitFor(() => expect(sent("open_notification_settings")).toHaveLength(1));
		}
	);

	it("says nothing while MaxHQ's own badge is off", async () => {
		access = { permission: "allowed", badge: false };
		setBadgePrefs({ show: false });
		await renderPage();
		await screen.findByText("macOS shows MaxHQ's notifications.");
		expect(card("Badge").queryByText(HIDDEN)).toBeNull();
	});

	it("drops the line once badges are back on in System Settings", async () => {
		access = { permission: "allowed", badge: false };
		await renderPage();
		await card("Badge").findByText(HIDDEN);
		access = { permission: "allowed", badge: true };
		act(() => {
			window.dispatchEvent(new Event("focus"));
		});
		await waitFor(() => expect(card("Badge").queryByText(HIDDEN)).toBeNull());
	});

	it("comes up in the search by its title, and still by Dock badge", () => {
		const page = { title: "Badge", terms: DOCK_BADGE_TERMS };
		expect(findIn(page, searchWords("badge"))?.title).toEqual([{ text: "Badge", hit: true }]);
		expect(findIn(page, searchWords("dock badge"))?.term).toEqual([
			{ text: "Dock", hit: true },
			{ text: " ", hit: false },
			{ text: "badge", hit: true },
		]);
	});
});

describe("in a browser", () => {
	it("shows the defaults and says there are none to show", async () => {
		delete app.isTauri;
		await renderPage();
		expect(checked("Sound for prompts")).toBe(true);
		expect(await screen.findByText(/doesn't run from an app bundle/)).toBeInTheDocument();
		expect(calls).toHaveLength(0);
	});
});
