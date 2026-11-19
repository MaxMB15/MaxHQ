// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sidebar's settings that belong to this Mac rather than a device:
// whether busy agents fold into a Working shelf, the snooze menu's
// choices, and whether the sidebar shows and how wide. They live in
// localStorage, which can throw or hold anything, so every read checks
// what it got and falls back to the defaults.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import {
	DEFAULT_SNOOZE_PRESETS,
	isSnoozePresetList,
	type SnoozePresetSetting,
} from "@maxhq/shared/view/snooze";

/** The storage key. */
export const PREFS_KEY = "maxhq.sidebar";

// Bump when the saved shape changes, so old settings read as the defaults.
const VERSION = 1;

/** The sidebar's own settings. */
export type SidebarPrefs = {
	/** Whether busy agents fold into a Working shelf at the bottom. */
	readonly workingShelf: boolean;
	/** The snooze menu's choices, at most eight. */
	readonly snoozePresets: readonly SnoozePresetSetting[];
	/** Whether the sidebar shows, or you hid it. */
	readonly open: boolean;
	/** The sidebar's width in pixels. */
	readonly width: number;
};

/** The narrowest the sidebar's edge drags to. */
export const MIN_WIDTH = 220;
/** The widest the sidebar's edge drags to. */
export const MAX_WIDTH = 480;
/** The sidebar's width until you drag it, and after a double-click on its edge. */
export const DEFAULT_WIDTH = 288;

/** The width held between the narrowest and the widest, in whole pixels. */
export const clampWidth = (width: number): number =>
	Math.round(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, width)));

/**
 * What the sidebar starts with. The Working shelf starts off in a sidebar
 * grouped by project, so busy agents stay in theirs.
 */
export const DEFAULT_PREFS: SidebarPrefs = {
	workingShelf: false,
	snoozePresets: DEFAULT_SNOOZE_PRESETS,
	open: true,
	width: DEFAULT_WIDTH,
};

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

/** Reads saved settings, keeping each part that checks out and defaulting the rest. */
export const readPrefs = (raw: string | null): SidebarPrefs => {
	if (raw === null) return DEFAULT_PREFS;
	let saved: unknown;
	try {
		saved = JSON.parse(raw);
	} catch {
		return DEFAULT_PREFS;
	}
	if (!isObject(saved) || saved.version !== VERSION) return DEFAULT_PREFS;
	const { workingShelf, snoozePresets, open, width } = saved;
	return {
		workingShelf: typeof workingShelf === "boolean" ? workingShelf : DEFAULT_PREFS.workingShelf,
		snoozePresets: isSnoozePresetList(snoozePresets)
			? snoozePresets
			: DEFAULT_PREFS.snoozePresets,
		open: typeof open === "boolean" ? open : DEFAULT_PREFS.open,
		width:
			typeof width === "number" && Number.isFinite(width)
				? clampWidth(width)
				: DEFAULT_PREFS.width,
	};
};

/** The settings saved on this Mac, or the defaults. */
export const loadPrefs = (): SidebarPrefs => {
	try {
		return readPrefs(localStorage.getItem(PREFS_KEY));
	} catch {
		return DEFAULT_PREFS;
	}
};

const savePrefs = (prefs: SidebarPrefs): void => {
	try {
		localStorage.setItem(PREFS_KEY, JSON.stringify({ version: VERSION, ...prefs }));
	} catch (error) {
		console.warn("Couldn't save the sidebar settings", error);
	}
};

const stores = new WeakMap<Services, StoreApi<SidebarPrefs>>();

/** The sidebar's settings for one window, read from storage the first time. */
export const prefsOf = (services: Services): StoreApi<SidebarPrefs> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<SidebarPrefs>(loadPrefs);
		stores.set(services, store);
	}
	return store;
};

/** Changes some of the settings and saves them all. */
export const setPrefs = (services: Services, change: Partial<SidebarPrefs>): void => {
	const store = prefsOf(services);
	store.setState(change);
	savePrefs(store.getState());
};

/**
 * Takes up changes another window saves, so every window's sidebar agrees.
 * Whether the sidebar shows and how wide stay this window's own, as its
 * panes do, and the last window to save them sets where a new one starts.
 * Returns a function that stops.
 */
export const watchPrefs = (services: Services): (() => void) => {
	const onStorage = (e: StorageEvent) => {
		if (e.key !== PREFS_KEY) return;
		const store = prefsOf(services);
		const { open, width } = store.getState();
		store.setState({ ...readPrefs(e.newValue), open, width });
	};
	window.addEventListener("storage", onStorage);
	return () => window.removeEventListener("storage", onStorage);
};

/** Reads the sidebar's settings. */
export const usePrefs = <T>(selector: (prefs: SidebarPrefs) => T): T =>
	useStore(prefsOf(useServices()), selector);
