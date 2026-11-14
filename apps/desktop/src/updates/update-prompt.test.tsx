// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { fake, fakeApp, leaveApp, sendStatus, sent } from "./testing";
import { UpdatePrompt } from "./update-prompt";

beforeEach(() => fakeApp());
afterEach(leaveApp);

const offered = async () => {
	render(<UpdatePrompt />);
	await sendStatus({ state: "available", version: "1.4.0" });
	await screen.findByRole("alertdialog");
};

describe("the update prompt", () => {
	it("doesn't show while there's nothing to install", async () => {
		render(<UpdatePrompt />);
		await sendStatus({ state: "up_to_date", checked_at: 0 });
		expect(screen.queryByRole("alertdialog")).toBeNull();
	});

	it("names the version and links to what's new in it", async () => {
		await offered();
		expect(screen.getByText("MaxHQ 1.4.0 is available")).toBeInTheDocument();
		expect(screen.getByRole("link", { name: "What's new" })).toHaveAttribute(
			"href",
			"https://example.com/OWNER/REPO/releases/tag/v1.4.0"
		);
	});

	it("leaves out What's new when the app doesn't know where releases are", async () => {
		fake.held = { ...fake.held, release_notes: null };
		await offered();
		expect(screen.queryByRole("link", { name: "What's new" })).toBeNull();
	});

	it("goes away on Later, until a newer version comes", async () => {
		await offered();
		fireEvent.click(screen.getByRole("button", { name: "Later" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		await sendStatus({ state: "available", version: "1.4.0" });
		expect(screen.queryByRole("alertdialog")).toBeNull();
		await sendStatus({ state: "available", version: "1.5.0" });
		expect(await screen.findByText("MaxHQ 1.5.0 is available")).toBeInTheDocument();
	});

	it("installs on Update now and shows the download's progress", async () => {
		await offered();
		await act(async () => {
			fireEvent.click(screen.getByRole("button", { name: "Update now" }));
			await Promise.resolve();
		});
		expect(sent("install_update")).toHaveLength(1);
		await sendStatus({ state: "downloading", version: "1.4.0", downloaded: 30, total: 100 });
		expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "30");
		expect(screen.getByText("Downloading version 1.4.0… 30%")).toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Later" })).toBeDisabled();
		expect(screen.getByRole("button", { name: "Update now" })).toBeDisabled();
	});

	it("says why an install failed, and offers to try again", async () => {
		await offered();
		await sendStatus({ state: "failed", reason: "bad signature", version: "1.4.0" });
		expect(
			screen.getByText("Couldn't install version 1.4.0: bad signature")
		).toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Try again" })).toBeEnabled();
	});
});
