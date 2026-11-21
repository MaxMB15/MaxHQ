// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { UpdateStatus } from "@maxhq/shared/bindings";

import {
	checkNow,
	later,
	progressOf,
	promptVersion,
	setAutoCheck,
	showPrompt,
	statusText,
	type UpdateState,
	updateNow,
	updates,
	versionButton,
	watchUpdates,
} from "./store";
import { fake, fakeApp, leaveApp, sendStatus, sent } from "./testing";

const state = (status: UpdateStatus, later: string | null = null): UpdateState => ({
	update: { version: "1.3.0", status, prefs: { auto_check: true }, release_notes: null },
	later,
	error: null,
});

describe("the prompt's version", () => {
	it("is the one on offer, while it downloads, installs or failed to", () => {
		expect(promptVersion(state({ state: "available", version: "1.4.0" }))).toBe("1.4.0");
		expect(
			promptVersion(
				state({ state: "downloading", version: "1.4.0", downloaded: 1, total: null })
			)
		).toBe("1.4.0");
		expect(promptVersion(state({ state: "installing", version: "1.4.0" }))).toBe("1.4.0");
		expect(
			promptVersion(state({ state: "failed", reason: "no space", version: "1.4.0" }))
		).toBe("1.4.0");
	});

	it("is none when there's nothing to install, or you said Later to it", () => {
		expect(promptVersion(state({ state: "idle" }))).toBeNull();
		expect(promptVersion(state({ state: "checking" }))).toBeNull();
		expect(promptVersion(state({ state: "up_to_date", checked_at: 0 }))).toBeNull();
		expect(
			promptVersion(state({ state: "failed", reason: "offline", version: null }))
		).toBeNull();
		expect(promptVersion(state({ state: "unavailable", reason: "disk_image" }))).toBeNull();
		expect(promptVersion(state({ state: "available", version: "1.4.0" }, "1.4.0"))).toBeNull();
		// A newer one than the one you put off asks again.
		expect(promptVersion(state({ state: "available", version: "1.5.0" }, "1.4.0"))).toBe(
			"1.5.0"
		);
		expect(promptVersion({ update: null, later: null, error: null })).toBeNull();
	});
});

describe("the status bar's update button", () => {
	it("checks while no update is known, and spins while the app looks", () => {
		expect(versionButton(state({ state: "idle" }))).toEqual({ kind: "check", checking: false });
		expect(versionButton(state({ state: "up_to_date", checked_at: 0 }))).toEqual({
			kind: "check",
			checking: false,
		});
		expect(versionButton(state({ state: "failed", reason: "offline", version: null }))).toEqual(
			{
				kind: "check",
				checking: false,
			}
		);
		expect(versionButton(state({ state: "checking" }))).toEqual({
			kind: "check",
			checking: true,
		});
	});

	it("updates while a version is on offer, even after Later", () => {
		const offer = { kind: "update", version: "1.4.0" };
		expect(versionButton(state({ state: "available", version: "1.4.0" }, "1.4.0"))).toEqual(
			offer
		);
		expect(
			versionButton(
				state({ state: "downloading", version: "1.4.0", downloaded: 1, total: null })
			)
		).toEqual(offer);
		expect(versionButton(state({ state: "installing", version: "1.4.0" }))).toEqual(offer);
		expect(
			versionButton(state({ state: "failed", reason: "no space", version: "1.4.0" }))
		).toEqual(offer);
	});

	it("is off in a copy that doesn't update itself, saying why", () => {
		expect(versionButton(state({ state: "unavailable", reason: "from_source" }))).toEqual({
			kind: "off",
			tip: "Updates come with release builds. This copy was built from source.",
		});
		expect(versionButton(state({ state: "unavailable", reason: "disk_image" }))).toMatchObject({
			kind: "off",
			tip: expect.stringContaining("disk image"),
		});
		expect(versionButton({ update: null, later: null, error: null })).toBeNull();
	});

	it("shows the prompt again after Later", () => {
		updates.setState(state({ state: "available", version: "1.4.0" }, "1.4.0"));
		expect(promptVersion(updates.getState())).toBeNull();
		showPrompt();
		expect(promptVersion(updates.getState())).toBe("1.4.0");
	});
});

describe("the status's words", () => {
	it("say how far the download is when its size is known", () => {
		const half: UpdateStatus = {
			state: "downloading",
			version: "1.4.0",
			downloaded: 50,
			total: 200,
		};
		expect(progressOf(half)).toBe(0.25);
		expect(statusText(half)).toBe("Downloading version 1.4.0… 25%");
		const unknown: UpdateStatus = { ...half, total: null };
		expect(progressOf(unknown)).toBeNull();
		expect(statusText(unknown)).toBe("Downloading version 1.4.0…");
	});

	it("say why a copy doesn't update, and why a step failed", () => {
		expect(statusText({ state: "unavailable", reason: "translocated" })).toContain(
			"temporary copy"
		);
		expect(statusText({ state: "failed", reason: "offline", version: null })).toBe(
			"Couldn't check for updates: offline"
		);
		expect(statusText({ state: "failed", reason: "bad signature", version: "1.4.0" })).toBe(
			"Couldn't install version 1.4.0: bad signature"
		);
	});
});

describe("the window's update state", () => {
	beforeEach(() => fakeApp());
	afterEach(leaveApp);

	it("reads the app's and follows each change it sends", async () => {
		const stop = watchUpdates();
		await waitFor(() => expect(updates.getState().update?.version).toBe("1.3.0"));
		await sendStatus({ state: "available", version: "1.4.0" });
		expect(updates.getState().update?.status).toEqual({
			state: "available",
			version: "1.4.0",
		});
		stop();
		// Stopped, it hears no more.
		await sendStatus({ state: "checking" });
		expect(updates.getState().update?.status.state).toBe("available");
	});

	it("checks, puts off and installs through the app", async () => {
		const stop = watchUpdates();
		await waitFor(() => expect(updates.getState().update).not.toBeNull());
		await act(() => checkNow());
		expect(sent("check_for_update")).toHaveLength(1);
		expect(promptVersion(updates.getState())).toBe("1.4.0");
		later("1.4.0");
		expect(promptVersion(updates.getState())).toBeNull();
		await act(() => updateNow());
		expect(sent("install_update")).toHaveLength(1);
		stop();
	});

	it("saves the switch, and puts it back when the save fails", async () => {
		const stop = watchUpdates();
		await waitFor(() => expect(updates.getState().update).not.toBeNull());
		await act(() => setAutoCheck(false));
		expect(sent("set_update_prefs")).toEqual([
			["set_update_prefs", { prefs: { auto_check: false } }],
		]);
		expect(updates.getState().update?.prefs.auto_check).toBe(false);

		fake.saveFails = "disk full";
		await act(() => setAutoCheck(true));
		expect(updates.getState().update?.prefs.auto_check).toBe(false);
		expect(updates.getState().error).toBe("Not saved: disk full");
		stop();
	});
});
