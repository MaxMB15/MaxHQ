// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The number on MaxHQ's icon in the Dock, and what counts toward it. The
// settings belong to this Mac, so they live in localStorage, which can
// throw or hold anything, and every read falls back to the defaults. The
// count itself is the shared one, so the badge and the inbox agree. In a
// browser there's no icon, so nothing is set. Windows has no badge count
// for an app like this one, so there the count is drawn into the small
// overlay icon on the taskbar button.

import { Image } from "@tauri-apps/api/image";
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

import { inApp, onWindows } from "./platform";

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

/** The overlay's width and height: twice the taskbar's 16 pixels, so it
 * stays sharp on a scaled display. */
const OVERLAY = 32;

/** What the overlay says for `count`: the count, or 9+ past nine, which is
 * all that fits. */
export const overlayText = (count: number): string => (count > 9 ? "9+" : String(count));

/** The overlay's pixels for `count`, white on a red disc, or null where
 * the page can't draw. */
const overlayPixels = (count: number): Uint8Array | null => {
	const canvas = document.createElement("canvas");
	canvas.width = OVERLAY;
	canvas.height = OVERLAY;
	const context = canvas.getContext("2d");
	if (!context) return null;
	const middle = OVERLAY / 2;
	context.fillStyle = "#c42b1c";
	context.beginPath();
	context.arc(middle, middle, middle, 0, 2 * Math.PI);
	context.fill();
	const text = overlayText(count);
	context.fillStyle = "#ffffff";
	context.font = `600 ${text.length > 1 ? 17 : 21}px "Segoe UI", sans-serif`;
	context.textAlign = "center";
	context.textBaseline = "middle";
	context.fillText(text, middle, middle + 1);
	return new Uint8Array(context.getImageData(0, 0, OVERLAY, OVERLAY).data);
};

/** Puts `count` on the taskbar button, or takes the overlay off at zero. */
const showOverlay = async (count: number): Promise<void> => {
	const window = getCurrentWindow();
	if (count <= 0) return window.setOverlayIcon(undefined);
	const pixels = overlayPixels(count);
	if (!pixels) return;
	const image = await Image.new(pixels, OVERLAY, OVERLAY);
	try {
		await window.setOverlayIcon(image);
	} finally {
		await image.close();
	}
};

/** The overlays in the order the count changed, so a slow one can't land
 * after the next. */
let overlays: Promise<void> = Promise.resolve();

/** Puts the count on the icon, or takes the badge off at zero. */
const showCount = (count: number): void => {
	if (!inApp()) return;
	const failed = (error: unknown) => console.warn("Couldn't set the Dock badge", error);
	try {
		if (onWindows()) {
			overlays = overlays.then(() => showOverlay(count)).catch(failed);
			return;
		}
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
