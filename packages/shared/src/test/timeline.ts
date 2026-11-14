// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A timeline for tests, with older pages that come a moment after they're
// asked for.

import type { TimelineItem, TimelineUpdate } from "../bindings";
import { applyUpdate } from "../timeline/apply";
import { timelineItem } from "../timeline/build";
import { createTimelineStore, type Timeline } from "../timeline/store";

/** A reply that reads as its ID unless it says otherwise. */
export const reply = (id: string, ordinal: number, text = id): TimelineItem =>
	timelineItem({ kind: "assistant_message", text, streaming: false }, { id, ordinal });

/**
 * A timeline whose older pages come from `pages`, newest first, or fail
 * with `failure` once they run out. `push` applies an update as maxd sends
 * it, and `loads` counts the asks for an older page.
 */
export const fakeTimeline = (pages: TimelineItem[][], failure?: string) => {
	const store = createTimelineStore();
	const push = (update: TimelineUpdate) =>
		store.setState(applyUpdate(store.getState(), update), true);
	let loads = 0;
	const loadOlder = () => {
		loads++;
		const state = store.getState();
		if (state.loadingOlder || !state.hasOlder || state.order.length === 0) return;
		store.setState({ loadingOlder: true });
		const page = pages.shift();
		void Promise.resolve().then(() => {
			if (page) push({ type: "older", items: page, has_older: pages.length > 0 });
			else store.setState({ loadingOlder: false, error: failure ?? "No page" });
		});
	};
	const timeline: Timeline = {
		store,
		loadOlder,
		reload: async () => {},
		close: () => {},
		onArrive: () => () => {},
	};
	return { timeline, push, loads: () => loads };
};
