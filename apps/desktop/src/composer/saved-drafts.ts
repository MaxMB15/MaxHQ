// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The drafts this Mac keeps through a restart, in the webview's
// localStorage, which every window shares. Each window writes what you
// typed soon after, and at once when it goes, and drops an agent's draft
// once the agent is deleted. The shared saved-drafts.ts tells what a draft
// keeps and for how long.

import type { Services } from "@/app/services";
import {
	createSavedDrafts,
	type DraftStorage,
	dropGoneDrafts,
	type SavedDrafts,
} from "@maxhq/shared/composer/saved-drafts";

/** The storage key. */
export const DRAFTS_KEY = "maxhq.drafts";

const storage: DraftStorage = {
	read: () => localStorage.getItem(DRAFTS_KEY),
	write: (text) => localStorage.setItem(DRAFTS_KEY, text),
};

const windows = new WeakMap<Services, SavedDrafts>();

/** The drafts saved on this Mac, as one window reaches them. */
export const draftsOf = (services: Services): SavedDrafts => {
	let drafts = windows.get(services);
	if (!drafts) {
		drafts = createSavedDrafts(storage);
		windows.set(services, drafts);
	}
	return drafts;
};

/**
 * Drops the drafts of agents the view loses, and writes what waits when
 * the page goes. Returns a function that stops, and writes what waits then
 * too.
 */
export const watchDrafts = (services: Services): (() => void) => {
	const drafts = draftsOf(services);
	const stop = dropGoneDrafts(services.view, drafts);
	window.addEventListener("pagehide", drafts.flush);
	return () => {
		stop();
		window.removeEventListener("pagehide", drafts.flush);
		drafts.flush();
	};
};
