// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the agent screen opens: Loading until the devices have answered, so
// a cold start from a push doesn't call the agent gone, and where the feed
// goes for the prompt it was opened for.

import type { AttentionId, Timestamp } from "@maxhq/shared/bindings";
import { stillSyncing } from "@maxhq/shared/status/reach";
import { type FeedRow, feedIndexOf } from "@maxhq/shared/timeline/output";
import type { ViewState } from "@maxhq/shared/view/replica";
import type { ViewStatus } from "@maxhq/shared/view/store";

/** What the screen says for an agent the view doesn't have. */
export const missingAgent = (
	status: ViewStatus,
	view: ViewState,
	now: Timestamp
): "loading" | "gone" =>
	status.state === "loading" || stillSyncing(view, now) ? "loading" : "gone";

/** Where the feed goes for a prompt: its card's row, the end, or nowhere yet. */
export type FeedScroll = number | "end" | null;

/**
 * The prompt's card once the feed has it. When the loaded feed has no card
 * for it, the end, where the card for an open prompt waits under the feed.
 */
export const feedScroll = (
	rows: readonly FeedRow[],
	attention: AttentionId | undefined,
	loaded: boolean
): FeedScroll => {
	if (attention === undefined) return null;
	const index = feedIndexOf(rows, attention);
	if (index >= 0) return index;
	return loaded ? "end" : null;
};
