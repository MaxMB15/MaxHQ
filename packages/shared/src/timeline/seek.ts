// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Loading a timeline until it has an item, before a view scrolls to the
// item's row. A quote jump and find both go to items the timeline may not
// have yet, so this waits for it to open and loads older pages until it
// does. A step to an earlier prompt loads until one shows up.

import type { ItemId } from "../bindings";
import type { TimelineState } from "./apply";
import type { Timeline } from "./store";

// Older pages a load asks for looking for the item before it gives up.
const MAX_PAGES = 50;

/** Why loading an item stopped short. */
export type LoadFailure =
	/** The timeline failed to open. */
	| "unopened"
	/** An older page failed to load. */
	| "unloaded"
	/** The timeline has no such item, or none that fits. */
	| "gone";

/**
 * Waits for the timeline to open, then loads older pages until `find`
 * picks an item from what's loaded, and calls `loaded` with it. Returns
 * the call that stops it.
 */
export const loadUntil = (
	timeline: Timeline,
	find: (state: TimelineState) => ItemId | null,
	loaded: (item: ItemId) => void,
	failed: (why: LoadFailure) => void
): (() => void) => {
	let pages = 0;
	let done = false;
	// The first item when the last page was asked for, to tell a failed load.
	let first: ItemId | undefined;
	let unsubscribe = () => {};
	const stop = () => {
		done = true;
		unsubscribe();
	};
	const fail = (why: LoadFailure) => {
		stop();
		failed(why);
	};
	const check = () => {
		if (done) return;
		const state = timeline.store.getState();
		if (state.status === "loading") return;
		if (state.status === "failed") return fail("unopened");
		const found = find(state);
		if (found !== null) {
			stop();
			return loaded(found);
		}
		if (state.loadingOlder) return;
		if (pages > 0 && state.order[0] === first) return fail("unloaded");
		if (!state.hasOlder || pages >= MAX_PAGES) return fail("gone");
		pages++;
		first = state.order[0];
		timeline.loadOlder();
	};
	unsubscribe = timeline.store.subscribe(check);
	check();
	return stop;
};

/**
 * Waits for the timeline to open, then loads older pages until it has the
 * item and calls `loaded`. Returns the call that stops it.
 */
export const loadItem = (
	timeline: Timeline,
	item: ItemId,
	loaded: () => void,
	failed: (why: LoadFailure) => void
): (() => void) =>
	loadUntil(timeline, (state) => (state.rows.includes(item) ? item : null), loaded, failed);
