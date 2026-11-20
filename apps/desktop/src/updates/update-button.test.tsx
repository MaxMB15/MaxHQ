// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { later, promptVersion, updates } from "./store";
import { fake, fakeApp, leaveApp, sendStatus, sent } from "./testing";
import { NOTE_MS, UpdateButton } from "./update-button";

afterEach(() => {
	vi.useRealTimers();
	leaveApp();
});

const check = () => screen.findByRole("button", { name: "Check for updates" });

describe("the button beside the version", () => {
	it("checks on a click, says it's up to date for a moment, then goes quiet", async () => {
		fakeApp({ state: "idle" });
		fake.found = { state: "up_to_date", checked_at: 0 };
		render(<UpdateButton />);
		const button = await check();
		expect(button.parentElement?.title).toBe("Check for updates");
		vi.useFakeTimers({ shouldAdvanceTime: true });
		await act(async () => fireEvent.click(button));
		expect(sent("check_for_update")).toHaveLength(1);
		expect(screen.getByRole("status")).toHaveTextContent("Up to date");
		act(() => vi.advanceTimersByTime(NOTE_MS));
		expect(screen.queryByRole("status")).toBeNull();
	});

	it("spins while the app checks, and takes no click then", async () => {
		fakeApp({ state: "idle" });
		render(<UpdateButton />);
		await check();
		await sendStatus({ state: "checking" });
		const button = screen.getByRole("button", { name: "Check for updates" });
		expect(button.getAttribute("aria-disabled")).toBe("true");
		expect(button.querySelector("svg")?.getAttribute("class")).toContain("animate-spin");
		fireEvent.click(button);
		expect(sent("check_for_update")).toHaveLength(0);
	});

	it("says quietly when a check fails, with why on hover", async () => {
		fakeApp({ state: "idle" });
		fake.found = { state: "failed", reason: "offline", version: null };
		render(<UpdateButton />);
		const button = await check();
		await act(async () => fireEvent.click(button));
		expect(screen.getByRole("status")).toHaveTextContent("Couldn't check");
		expect(button.parentElement?.title).toBe("Couldn't check for updates: offline");
		expect(screen.queryByRole("dialog")).toBeNull();
	});

	it("becomes the Update button when a check finds one, which opens the prompt again", async () => {
		fakeApp({ state: "idle" });
		render(<UpdateButton />);
		await act(async () => fireEvent.click(await check()));
		const update = screen.getByRole("button", { name: "Update" });
		expect(update.title).toBe("Update to version 1.4.0");
		later("1.4.0");
		expect(promptVersion(updates.getState())).toBeNull();
		fireEvent.click(update);
		expect(promptVersion(updates.getState())).toBe("1.4.0");
		expect(sent("install_update")).toHaveLength(0);
	});

	it("is off in a copy built from source, and says updates come with release builds", async () => {
		fakeApp({ state: "unavailable", reason: "from_source" });
		render(<UpdateButton />);
		const button = await check();
		expect(button.getAttribute("aria-disabled")).toBe("true");
		expect(button.parentElement?.title).toBe(
			"Updates come with release builds. This copy was built from source."
		);
		fireEvent.click(button);
		expect(sent("check_for_update")).toHaveLength(0);
	});
});
