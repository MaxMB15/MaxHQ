// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Find in an agent's feed. The shared find asks maxd and steps among the
// matches, as the desktop's bar does. Each move to a match loads older
// pages until the timeline has its item, then names the feed row to go to
// and mark. Closing clears the mark and leaves the feed where it is.

import { createStore, type StoreApi } from "zustand/vanilla";

import type { AgentId } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { createFind, type Find } from "@maxhq/shared/find/store";
import { followTimeline, timelineAsk } from "@maxhq/shared/find/timeline";
import { feedRowFor } from "@maxhq/shared/timeline/output";
import { loadItem } from "@maxhq/shared/timeline/seek";
import type { Timeline } from "@maxhq/shared/timeline/store";

/** The feed row find marks, or null, and a count that goes up each time the feed should go to it. */
export type FeedGoal = { readonly row: string | null; readonly turn: number };

/** One agent screen's find. */
export type FeedFind = {
	readonly find: Find;
	readonly goal: StoreApi<FeedGoal>;
	/** Starts asking again as the timeline changes and going to each match. Returns the stop. */
	readonly follow: () => () => void;
};

export const createFeedFind = (
	source: Pick<DataSource, "findInTimeline">,
	agent: AgentId,
	timeline: Timeline
): FeedFind => {
	const find = createFind(timelineAsk(source, agent));
	const goal = createStore<FeedGoal>(() => ({ row: null, turn: 0 }));
	let stopLoad = () => {};
	const clear = () => {
		stopLoad();
		if (goal.getState().row !== null) goal.setState({ row: null });
	};

	const follow = () => {
		const unfollow = followTimeline(find, timeline);
		const unsubscribe = find.store.subscribe((state, before) => {
			if (!state.open || !state.match) return clear();
			if (state.turn === before.turn) return;
			const { item } = state.match;
			stopLoad();
			stopLoad = loadItem(
				timeline,
				item,
				() =>
					goal.setState(({ turn }) => ({
						row: feedRowFor(timeline.store.getState(), item),
						turn: turn + 1,
					})),
				// The bar still counts the match when its row can't be reached.
				clear
			);
		});
		return () => {
			unsubscribe();
			unfollow();
			stopLoad();
			find.dispose();
		};
	};

	return { find, goal, follow };
};
