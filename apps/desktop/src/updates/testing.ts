// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pretend app for the update tests: it answers the update commands from
// what it holds, keeps the calls made, and sends status changes the way
// the app does.

import type { InvokeArgs } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { act } from "@testing-library/react";

import type { AppUpdate, UpdatePrefs, UpdateStatus } from "@maxhq/shared/bindings";

import { resetUpdates } from "./store";

const app = globalThis as { isTauri?: boolean };

/** What the pretend app holds, and the calls made to it. */
export const fake = {
	held: null as unknown as AppUpdate,
	calls: [] as [string, InvokeArgs | undefined][],
	/** What Check now finds. */
	found: { state: "available", version: "1.4.0" } as UpdateStatus,
	/** Why saving the setting fails, when it does. */
	saveFails: null as string | null,
};

/** Sets up the pretend app holding `status`. */
export const fakeApp = (status: UpdateStatus = { state: "idle" }): void => {
	app.isTauri = true;
	resetUpdates();
	fake.held = {
		version: "1.3.0",
		status,
		prefs: { auto_check: true },
		release_notes: "https://example.com/OWNER/REPO/releases",
	};
	fake.calls = [];
	fake.found = { state: "available", version: "1.4.0" };
	fake.saveFails = null;
	mockIPC(
		(command, args) => {
			fake.calls.push([command, args]);
			switch (command) {
				case "app_update":
					return fake.held;
				case "check_for_update":
					fake.held = { ...fake.held, status: fake.found };
					return fake.found;
				case "install_update":
					return null;
				case "set_update_prefs":
					if (fake.saveFails) throw { kind: "failed", message: fake.saveFails };
					fake.held = { ...fake.held, prefs: (args as { prefs: UpdatePrefs }).prefs };
					return null;
			}
		},
		{ shouldMockEvents: true }
	);
};

/** Undoes `fakeApp`. */
export const leaveApp = (): void => {
	resetUpdates();
	clearMocks();
	delete app.isTauri;
};

/** Sends a status change, as the app does. */
export const sendStatus = (status: UpdateStatus) =>
	act(async () => {
		fake.held = { ...fake.held, status };
		await emit("app-update-changed", status);
	});

/** The calls made to `command`. */
export const sent = (command: string) => fake.calls.filter(([name]) => name === command);
