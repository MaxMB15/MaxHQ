// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which of the sidebar's shelves are open, kept on this Mac so the next
// launch opens the ones you left open. Like the sidebar's settings, they
// live in localStorage, which can throw or hold anything, so every read
// checks what it got and falls back to every shelf closed.

import type { Services } from "@/app/services";
import type { Shelf } from "@maxhq/shared/ui/store";

/** The storage key. */
export const SHELVES_KEY = "maxhq.shelves";

// Bump when the saved shape changes, so old saves read as every shelf closed.
const VERSION = 1;

// Every shelf, in the order they're saved.
const SHELVES: readonly Shelf[] = ["working", "snoozed", "settled"];

const CLOSED: ReadonlySet<Shelf> = new Set();

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

/** Reads the saved open shelves, keeping the ones it knows. */
export const readShelves = (raw: string | null): ReadonlySet<Shelf> => {
	if (raw === null) return CLOSED;
	let saved: unknown;
	try {
		saved = JSON.parse(raw);
	} catch {
		return CLOSED;
	}
	if (!isObject(saved) || saved.version !== VERSION) return CLOSED;
	const { open } = saved;
	if (!Array.isArray(open)) return CLOSED;
	return new Set(SHELVES.filter((shelf) => open.includes(shelf)));
};

/** The shelves saved open on this Mac, or none. */
export const loadShelves = (): ReadonlySet<Shelf> => {
	try {
		return readShelves(localStorage.getItem(SHELVES_KEY));
	} catch {
		return CLOSED;
	}
};

const saveShelves = (open: ReadonlySet<Shelf>): void => {
	try {
		localStorage.setItem(
			SHELVES_KEY,
			JSON.stringify({ version: VERSION, open: SHELVES.filter((shelf) => open.has(shelf)) })
		);
	} catch (error) {
		console.warn("Couldn't save which shelves are open", error);
	}
};

/**
 * Saves each shelf you open or close, and takes up the ones another window
 * saves, so every window's sidebar agrees. Returns a function that stops.
 */
export const watchShelves = (services: Services): (() => void) => {
	const stopSaving = services.ui.subscribe((state, before) => {
		if (state.openShelves !== before.openShelves) saveShelves(state.openShelves);
	});
	const onStorage = (e: StorageEvent) => {
		if (e.key === SHELVES_KEY) services.ui.setState({ openShelves: readShelves(e.newValue) });
	};
	window.addEventListener("storage", onStorage);
	return () => {
		stopSaving();
		window.removeEventListener("storage", onStorage);
	};
};
