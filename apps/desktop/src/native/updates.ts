// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The app's own update: its version, where the update stands, your setting
// for it, and the calls that look for one and install it. The app does the
// work, and says each time the status changes. In a browser there's no app
// to update.

import type { AppUpdate, NoUpdates, UpdatePrefs, UpdateStatus } from "@maxhq/shared/bindings";
import { commands, events } from "@/data/tauri-commands";

import { unwrap } from "./failure";
import { inApp } from "./platform";

/** What a page outside the app shows: nothing to update. */
const OUTSIDE: AppUpdate = {
	version: "",
	status: { state: "unavailable", reason: "from_source" },
	prefs: { auto_check: true },
	release_notes: null,
};

/** The version, the status and your setting, as the app holds them now. */
export const appUpdate = (): Promise<AppUpdate> =>
	inApp() ? commands.appUpdate() : Promise.resolve(OUTSIDE);

/** Looks for a newer version now, and resolves with what the app found. */
export const checkForUpdate = (): Promise<UpdateStatus> =>
	inApp() ? unwrap(commands.checkForUpdate()) : Promise.resolve(OUTSIDE.status);

/**
 * Downloads and installs the version found, then restarts the app. It
 * resolves only if the app doesn't restart, and rejects when that failed.
 */
export const installUpdate = (): Promise<void> =>
	inApp() ? unwrap(commands.installUpdate()).then(() => undefined) : Promise.resolve();

/** Saves the update setting. */
export const setUpdatePrefs = (prefs: UpdatePrefs): Promise<void> =>
	inApp() ? unwrap(commands.setUpdatePrefs(prefs)).then(() => undefined) : Promise.resolve();

/**
 * Calls `onChange` with each new status. Resolves once it listens, with a
 * function that stops.
 */
export const onUpdateChanged = async (
	onChange: (status: UpdateStatus) => void
): Promise<() => void> => {
	if (!inApp()) return () => undefined;
	const stop = await events.appUpdateChanged.listen((event) => onChange(event.payload));
	// Stopping is async, and a window going away may have lost its events
	// already.
	return () =>
		void Promise.resolve()
			.then(stop)
			.catch(() => undefined);
};

export type { AppUpdate, NoUpdates, UpdatePrefs, UpdateStatus };
