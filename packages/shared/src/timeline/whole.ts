// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The whole of an open timeline: waits for its first page, then asks for
// older pages one at a time until there are none, for an export.

import { t } from "../i18n";

import type { TimelineState } from "./apply";
import type { Timeline } from "./store";

/**
 * Resolves with the timeline's state once every older page has come in.
 * Rejects with the reason when it doesn't open or a page fails to load.
 */
export const loadAll = (timeline: Timeline): Promise<TimelineState> =>
	new Promise((resolve, reject) => {
		const { store } = timeline;
		// Whether a page was asked for, so an error from before doesn't count.
		let asked = false;
		let stop: (() => void) | null = null;
		let settled = false;
		const finish = (run: () => void) => {
			settled = true;
			stop?.();
			run();
		};
		// Settles, asks for the next page, or waits, by the state.
		const step = (state: TimelineState): void => {
			if (settled || state.status === "loading" || state.loadingOlder) return;
			if (state.status === "failed") {
				finish(() => reject(new Error(state.error ?? t("The conversation didn't open."))));
				return;
			}
			if (asked && state.error !== null) {
				const error = state.error;
				finish(() => reject(new Error(error)));
				return;
			}
			if (!state.hasOlder) {
				finish(() => resolve(state));
				return;
			}
			asked = true;
			timeline.loadOlder();
			// A timeline with nothing to ask before stays as it is.
			const after = store.getState();
			if (!after.loadingOlder && !settled) finish(() => resolve(after));
		};
		stop = store.subscribe(step);
		step(store.getState());
	});
