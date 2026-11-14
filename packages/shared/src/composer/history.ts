// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the entries, oldest
// first, with a prompt sent twice in a row kept once under the newer
// message, and the step back or forward, which only goes on while the box
// still holds what it brought back. Keeping the draft for Down to put
// back, starting a walk from the start of a box with text in it, and
// waiting for the queue are MaxHQ's.

// The messages Up and Down walk through in a composer: what you sent this
// agent, read from the timeline the view already has. Up from the very
// start of the box brings back the newest and keeps what was typed. From
// there Up and Down move a message at a time while the caret is on the
// first or last line, and Down past the newest puts the draft back.

import type { ItemId, TimelineItem } from "../bindings";
import type { Segment } from "../pills/draft";
import type { TimelineState } from "../timeline/apply";
import { userText } from "../timeline/export";

/** A message that went to the agent. */
export type SentMessage = Extract<TimelineItem, { kind: "user_message" }>;

/** A message Up can bring back. */
export type HistoryEntry = {
	readonly id: ItemId;
	/** The message as plain text, which tells a repeat. */
	readonly prompt: string;
	readonly message: SentMessage;
};

/** Where a walk is. */
export type HistoryPosition = {
	/** The entry in the box. */
	readonly entryId: ItemId;
	/** Its prompt, to find it by when a newer repeat takes its place. */
	readonly prompt: string;
	/** The box's text once it went in. The walk goes on while the box still has it. */
	readonly recalled: string;
	/** What the box held when the walk started, which Down past the newest puts back. */
	readonly draft: readonly Segment[];
};

/** Up walks back, to older messages, and Down forward. */
export type HistoryDirection = "backward" | "forward";

/** Where the caret is, as a step needs it. Both are false while anything is selected. */
export type HistoryCaret = {
	/** The caret is before everything in the box. */
	readonly atStart: boolean;
	/** The caret is on the line the key leaves the box from: the first for Up, the last for Down. */
	readonly onEdge: boolean;
};

/** What a step puts in the box: an entry, or the draft the walk kept. */
export type HistoryStep =
	| { readonly type: "entry"; readonly entry: HistoryEntry; readonly draft: readonly Segment[] }
	| { readonly type: "draft"; readonly draft: readonly Segment[] };

/**
 * The messages you sent, oldest first: each one the agent took, at once or
 * steered into a turn, as long as it has any words or files. One that says
 * what the one before it said is kept once, under the newer.
 */
export const historyEntries = (timeline: Pick<TimelineState, "items" | "order">) => {
	const entries: HistoryEntry[] = [];
	for (const id of timeline.order) {
		const message = timeline.items.get(id);
		if (message?.kind !== "user_message") continue;
		if (message.delivery !== "sent" && message.delivery !== "steer") continue;
		const prompt = userText(message);
		if (prompt === "") continue;
		if (entries.at(-1)?.prompt === prompt) {
			entries[entries.length - 1] = { id, prompt, message };
			continue;
		}
		entries.push({ id, prompt, message });
	}
	return entries;
};

const findActive = (entries: readonly HistoryEntry[], position: HistoryPosition): number => {
	const byId = entries.findIndex((entry) => entry.id === position.entryId);
	if (byId >= 0) return byId;
	for (let at = entries.length - 1; at >= 0; at--)
		if (entries[at]?.prompt === position.prompt) return at;
	return -1;
};

/**
 * One step through the entries from `position`, or null when the key
 * should do what it does anyway. The walk goes on only while the box has
 * the text it last put in, so an edit makes it a draft again. A walk
 * starts with Up from the very start of the box, once nothing waits in the
 * queue, and keeps what the box held. Down past the newest entry gives
 * that back.
 */
export const stepHistory = (input: {
	readonly direction: HistoryDirection;
	/**
	 * The entries, read only once the caret allows a step: built on the
	 * keypress, since the timeline changes with every streamed reply.
	 */
	readonly entries: () => readonly HistoryEntry[];
	readonly position: HistoryPosition | null;
	/** The box's text, each pill as one object mark. */
	readonly text: string;
	/** The box's text and pills, kept when a walk starts. */
	readonly segments: readonly Segment[];
	readonly caret: HistoryCaret;
	/** Whether messages wait in the queue, which Up takes back first. */
	readonly queued: boolean;
}): HistoryStep | null => {
	const { direction, position, caret } = input;
	const walking = position !== null && position.recalled === input.text ? position : null;
	const starts = direction === "backward" && !input.queued && caret.atStart;
	if (!(walking && caret.onEdge) && !starts) return null;
	const entries = input.entries();
	const index = walking ? findActive(entries, walking) : -1;
	if (!walking || index < 0) {
		const newest = entries.at(-1);
		return starts && newest ? { type: "entry", entry: newest, draft: input.segments } : null;
	}
	if (!caret.onEdge) return null;
	const entry = entries[direction === "backward" ? index - 1 : index + 1];
	if (entry) return { type: "entry", entry, draft: walking.draft };
	return direction === "forward" ? { type: "draft", draft: walking.draft } : null;
};
