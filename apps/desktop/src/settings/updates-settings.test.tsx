// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { UpdateStatus } from "@maxhq/shared/bindings";
import { fakeApp, leaveApp, sendStatus, sent } from "@/updates/testing";

import { UpdatesSettingsPage } from "./updates-settings";

afterEach(leaveApp);

const renderPage = async (status?: UpdateStatus) => {
	fakeApp(status);
	render(<UpdatesSettingsPage />);
	await screen.findByText("Version 1.3.0");
};

const autoCheck = () => screen.getByRole("switch", { name: "Check for updates automatically" });

describe("the Updates page", () => {
	it("shows the version, and checks now when asked", async () => {
		await renderPage({ state: "up_to_date", checked_at: 0 });
		expect(screen.getByRole("status")).toHaveTextContent("MaxHQ is up to date.");
		expect(autoCheck()).toHaveAttribute("aria-checked", "true");
		await act(async () => {
			fireEvent.click(screen.getByRole("button", { name: "Check now" }));
			await Promise.resolve();
		});
		expect(sent("check_for_update")).toHaveLength(1);
		expect(await screen.findByText("Version 1.4.0 is available.")).toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Update now" })).toBeEnabled();
	});

	it("follows the app while it checks", async () => {
		await renderPage();
		expect(screen.getByRole("status")).toHaveTextContent("hasn't checked");
		await sendStatus({ state: "checking" });
		expect(screen.getByRole("status")).toHaveTextContent("Checking for updates…");
		expect(screen.getByRole("button", { name: "Check now" })).toBeDisabled();
	});

	it("saves the switch", async () => {
		await renderPage();
		await act(async () => {
			fireEvent.click(autoCheck());
			await Promise.resolve();
		});
		expect(sent("set_update_prefs")).toEqual([
			["set_update_prefs", { prefs: { auto_check: false } }],
		]);
		expect(autoCheck()).toHaveAttribute("aria-checked", "false");
	});

	it("says why a copy outside Applications doesn't update", async () => {
		await renderPage({ state: "unavailable", reason: "not_in_applications" });
		expect(screen.getByRole("status")).toHaveTextContent(
			"MaxHQ updates itself only from the Applications folder."
		);
		expect(screen.getByRole("button", { name: "Check now" })).toBeDisabled();
		expect(autoCheck()).toHaveAttribute("data-disabled");
	});

	it("says what failed", async () => {
		await renderPage({ state: "failed", reason: "offline", version: null });
		expect(screen.getByRole("status")).toHaveTextContent("Couldn't check for updates: offline");
		expect(screen.getByRole("button", { name: "Check now" })).toBeEnabled();
	});
});
