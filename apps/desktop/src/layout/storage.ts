// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where each window keeps its pane layout: localStorage, under the window's
// label, so two windows keep two layouts. Storage can throw or hold anything,
// so every read checks what it got and treats anything odd as no layout.

import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { SerializedDockview } from "dockview-react";

// Bump when the saved shape changes, so an old layout reads as none.
const VERSION = 1;

/** The storage key for a window's layout. */
export const layoutKey = (label: string): string => `maxhq.layout.${label}`;

/** The label of the window this runs in, or "main" outside Tauri. */
export const windowLabel = (): string => {
	if (!isTauri()) return "main";
	try {
		return getCurrentWindow().label;
	} catch {
		return "main";
	}
};

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

/** Whether a value has the shape dockview writes, with every pane named by an ID. */
export const isLayout = (value: unknown): value is SerializedDockview => {
	if (!isObject(value) || !isObject(value.grid) || !isObject(value.panels)) return false;
	const { grid, panels } = value;
	if (!isObject(grid.root) || typeof grid.width !== "number" || typeof grid.height !== "number") {
		return false;
	}
	return Object.entries(panels).every(
		([id, panel]) =>
			isObject(panel) && panel.id === id && typeof panel.contentComponent === "string"
	);
};

/** Forgets a window's layout. */
export const clearLayout = (label: string): void => {
	try {
		localStorage.removeItem(layoutKey(label));
	} catch {}
};

/**
 * The layout a window saved, or null when it has none. One that can't be
 * read is removed, so the next start doesn't trip on it again.
 */
export const loadLayout = (label: string): SerializedDockview | null => {
	let raw: string | null;
	try {
		raw = localStorage.getItem(layoutKey(label));
	} catch {
		return null;
	}
	if (raw === null) return null;
	try {
		const saved: unknown = JSON.parse(raw);
		if (isObject(saved) && saved.version === VERSION && isLayout(saved.dock)) return saved.dock;
	} catch {}
	clearLayout(label);
	return null;
};

/** Saves a window's layout. A full or blocked storage loses it, which only costs the layout. */
export const saveLayout = (label: string, dock: SerializedDockview): void => {
	try {
		localStorage.setItem(layoutKey(label), JSON.stringify({ version: VERSION, dock }));
	} catch (error) {
		console.warn("Couldn't save the pane layout", error);
	}
};
