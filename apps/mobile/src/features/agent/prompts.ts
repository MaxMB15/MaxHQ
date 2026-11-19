// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Your prompts sheet: the prompts you sent the agent, newest first, to
// jump the feed to, as the desktop's rail beside the conversation offers
// them. The one nearest the top of the feed shows picked. While older pages
// wait to load, nothing counts the prompts, as on the desktop.

import { itemLine, promptRows, promptTotal } from "@maxhq/shared/agent-view/prompt-jump";
import type { ItemId, TimelineItem } from "@maxhq/shared/bindings";
import type { FeedRow } from "@maxhq/shared/timeline/output";

/** One prompt in the sheet. */
export type PromptEntry = {
	readonly id: ItemId;
	/** Its place among the feed's rows. */
	readonly row: number;
	/** Its number from the first prompt, or null while older pages may hold more. */
	readonly number: number | null;
	/** Its first line. */
	readonly line: string;
};

/** The prompts among the feed's rows, newest first. */
export const promptEntries = (
	rows: readonly FeedRow[],
	items: ReadonlyMap<ItemId, TimelineItem>,
	older: boolean
): PromptEntry[] => {
	const found = promptRows(
		rows.map((row) => row.id),
		items
	);
	const total = promptTotal(found.length, older);
	return found
		.map(({ id, row }, index) => ({
			id,
			row,
			number: total === null ? null : index + 1,
			line: itemLine(items.get(id)),
		}))
		.reverse();
};

/**
 * The prompt the feed is at: the last one at or above `top`, the row at the
 * top of the screen, else the first one loaded. Before the feed says which
 * row that is, the newest. Null with no prompts.
 */
export const promptAtTop = (
	entries: readonly PromptEntry[],
	rows: readonly FeedRow[],
	top: string | undefined
): ItemId | null => {
	const at = top === undefined ? -1 : rows.findIndex((row) => row.id === top);
	if (at < 0) return entries[0]?.id ?? null;
	return (entries.find((entry) => entry.row <= at) ?? entries.at(-1))?.id ?? null;
};
