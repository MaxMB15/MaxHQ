// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { NotificationPermission, NotificationPrefs } from "@maxhq/shared/bindings";
import { DEFAULT_NOTIFICATION_PREFS } from "@/native/notifications";

import { NotificationsSettingsPage } from "./notifications-settings";

const app = globalThis as { isTauri?: boolean };

/** What the app holds, and the calls the page made. */
let held: NotificationPrefs;
let permission: NotificationPermission;
const calls: [string, InvokeArgs | undefined][] = [];

beforeEach(() => {
	app.isTauri = true;
	held = { ...DEFAULT_NOTIFICATION_PREFS, kinds: [...DEFAULT_NOTIFICATION_PREFS.kinds] };
	permission = "allowed";
	calls.length = 0;
	mockIPC((command, args) => {
		calls.push([command, args]);
		switch (command) {
			case "notification_prefs":
				return held;
			case "set_notification_prefs":
				held = (args as { prefs: NotificationPrefs }).prefs;
				return null;
			case "notification_permission":
				return permission;
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

const renderPage = async () => {
	render(<NotificationsSettingsPage />);
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
		permission = "denied";
		await renderPage();
		expect(await screen.findByRole("status")).toHaveTextContent(
			"turned off for MaxHQ in System Settings"
		);
		fireEvent.click(screen.getByRole("button", { name: "Open System Settings" }));
		await waitFor(() => expect(sent("open_notification_settings")).toHaveLength(1));
	});

	it("reads macOS again when the window comes back", async () => {
		permission = "not_decided";
		await renderPage();
		expect(await screen.findByRole("status")).toHaveTextContent("hasn't asked");
		permission = "allowed";
		act(() => {
			window.dispatchEvent(new Event("focus"));
		});
		expect(await screen.findByText("macOS shows MaxHQ's notifications.")).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Open System Settings" })).toBeNull();
	});

	it("says when the build can't show any, and can't send a test", async () => {
		permission = "unavailable";
		await renderPage();
		const status = await screen.findByText(/doesn't run from an app bundle/);
		expect(
			within(status.closest("section")!).getByRole("button", {
				name: "Send a test notification",
			})
		).toBeDisabled();
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
