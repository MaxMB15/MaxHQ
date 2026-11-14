// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The drafts this phone keeps through a restart, in a file in the app's
// documents. They're written soon after you type and at once when the app
// leaves the screen, and an agent's draft goes once the agent is deleted.
// The shared saved-drafts.ts tells what a draft keeps and for how long.

import { File, Paths } from "expo-file-system";
import { AppState } from "react-native";

import { createSavedDrafts, dropGoneDrafts } from "@maxhq/shared/composer/saved-drafts";
import type { ViewStore } from "@maxhq/shared/view/store";

const kept = () => new File(Paths.document, "drafts.json");

/** The drafts saved on this phone. */
export const savedDrafts = createSavedDrafts({
	read: () => {
		const file = kept();
		return file.exists ? file.textSync() : null;
	},
	write: (text) => kept().write(text),
});

/**
 * Drops the drafts of agents the view loses, and writes what waits when the
 * app leaves the screen. Returns a function that stops, and writes then too.
 */
export const watchDrafts = (view: ViewStore): (() => void) => {
	const stop = dropGoneDrafts(view, savedDrafts);
	const watch = AppState.addEventListener("change", (state) => {
		if (state !== "active") savedDrafts.flush();
	});
	return () => {
		stop();
		watch.remove();
		savedDrafts.flush();
	};
};
