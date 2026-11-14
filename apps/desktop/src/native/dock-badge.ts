// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The number on MaxHQ's icon in the Dock, and what counts toward it. The
// settings belong to this Mac, so they live in localStorage, which can
// throw or hold anything, and every read falls back to the defaults. The
// count itself is the shared one, so the badge and the inbox agree. In a
// browser there's no icon, so nothing is set.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { Services } from "@/app/services";
import {
	type BadgePrefs,
	DEFAULT_BADGE_PREFS,
	readBadgePrefs,
	watchBadge,
	writeBadgePrefs,
} from "@maxhq/shared/view/badge";

import { inApp } from "./platform";

/** The storage key. */
export const BADGE_KEY = "maxhq.dock-badge";

const load = (): BadgePrefs => {
	try {
		return readBadgePrefs(localStorage.getItem(BADGE_KEY));
	} catch {
		return DEFAULT_BADGE_PREFS;
	}
};

const store = createStore<BadgePrefs>(load);

/** Changes some of the settings and saves them all. */
export const setBadgePrefs = (change: Partial<BadgePrefs>): void => {
	store.setState(change);
	try {
		localStorage.setItem(BADGE_KEY, writeBadgePrefs(store.getState()));
	} catch (error) {
		console.warn("Couldn't save the Dock badge settings", error);
	}
};

/** Reads the badge's settings. */
export const useBadgePrefs = <T>(selector: (prefs: BadgePrefs) => T): T =>
	useStore(store, selector);

/** Puts the count on the icon, or takes the badge off at zero. */
const showCount = (count: number): void => {
	if (!inApp()) return;
	const failed = (error: unknown) => console.warn("Couldn't set the Dock badge", error);
	try {
		// Throws at once when the page has no window behind it, as in tests.
		getCurrentWindow()
			.setBadgeCount(count > 0 ? count : undefined)
			.catch(failed);
	} catch (error) {
		failed(error);
	}
};

/**
 * Keeps the icon's badge current for as long as the window runs, taking up
 * settings another window saves. Returns a function that stops.
 */
export const watchDockBadge = (services: Services): (() => void) => {
	const onStorage = (e: StorageEvent) => {
		if (e.key === BADGE_KEY) store.setState(readBadgePrefs(e.newValue));
	};
	window.addEventListener("storage", onStorage);
	const stop = watchBadge({ view: services.view, prefs: store, set: showCount });
	return () => {
		window.removeEventListener("storage", onStorage);
		stop();
	};
};
