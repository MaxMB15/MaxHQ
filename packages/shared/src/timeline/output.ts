// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An agent's output for a screen too small for the desktop's timeline: its
// last lines as text, or the feed, one row per item the timeline shows, each
// knowing its kind and the space above it, so a row can read its own item
// from the store and redraw alone.

import { t } from "../i18n";

import type { AttentionId, FileEdit, TimelineItem } from "../bindings";
import {
	gapAbove,
	handoffLine,
	openedLine,
	sentFilesLine,
	toolTarget,
	visualLine,
} from "../agent-view/format";
import { sentPieces, sentText } from "../pills/sent";
import type { TimelineState } from "./apply";

/** Who a line is from. */
export type OutputSpeaker = "you" | "agent" | "tool" | "note";

/** One line of output. */
export type OutputLine = {
	/** The item it comes from. */
	readonly id: string;
	readonly speaker: OutputSpeaker;
	readonly text: string;
};

const TOOL_VERB: Partial<Record<TimelineItem["kind"], string>> = {
	get command() {
		return t("Ran");
	},
	get file_change() {
		return t("Edited");
	},
	get file_read() {
		return t("Read");
	},
	get search() {
		return t("Searched");
	},
	get web() {
		return t("Fetched");
	},
	get tool_call() {
		return t("Called");
	},
};

// A message you sent as text, each file it placed as its marker.
const sentLine = (item: Extract<TimelineItem, { kind: "user_message" }>): string => {
	const files = new Map(item.attachments.map((file) => [file.id, file]));
	return sentText(sentPieces(item), (id) => files.get(id));
};

// The line an item reads as, or null for one that shows nothing here.
const lineOf = (item: TimelineItem): Omit<OutputLine, "id"> | null => {
	const verb = TOOL_VERB[item.kind];
	if (verb) return { speaker: "tool", text: `${verb} ${toolTarget(item) ?? ""}`.trim() };
	switch (item.kind) {
		case "user_message":
			return { speaker: "you", text: sentLine(item) };
		case "assistant_message":
			return item.text ? { speaker: "agent", text: item.text } : null;
		case "agent_attachments":
			return {
				speaker: "agent",
				text: item.text || sentFilesLine(item.attachments.length),
			};
		case "plan":
			return { speaker: "agent", text: item.markdown };
		case "subagent":
			return {
				speaker: "tool",
				text: t("Started a subagent: {{item_description}}", {
					item_description: item.description,
				}),
			};
		case "approval":
			return { speaker: "note", text: item.title };
		case "question":
			return { speaker: "note", text: item.questions[0]?.text ?? t("Asked a question") };
		case "error":
		case "notice":
			return { speaker: "note", text: item.message };
		case "interrupted":
			return { speaker: "note", text: t("Interrupted") };
		case "handoff":
			return { speaker: "note", text: handoffLine(item) };
		case "agent_opened_file":
			return { speaker: "tool", text: openedLine(item) };
		case "agent_visual":
			return { speaker: "tool", text: visualLine(item) };
		default:
			return null;
	}
};

/** The last `count` lines of an agent's output, oldest first. */
export const recentOutput = (state: TimelineState, count = 40): OutputLine[] => {
	const lines: OutputLine[] = [];
	for (let i = state.rows.length - 1; i >= 0 && lines.length < count; i--) {
		const id = state.rows[i]!;
		const item = state.items.get(id);
		if (!item || item.parent !== null) continue;
		const line = lineOf(item);
		if (line) lines.push({ id, ...line });
	}
	return lines.reverse();
};

/** An approval or question in the feed, drawn as a card. */
export type FeedCard = Extract<TimelineItem, { kind: "approval" } | { kind: "question" }>;

/** Whether an item is a card. */
export const isCard = (item: TimelineItem): item is FeedCard =>
	item.kind === "approval" || item.kind === "question";

/** One row of the feed. */
export type FeedRow = {
	readonly id: string;
	readonly kind: TimelineItem["kind"];
	/** The space above it, in pixels. */
	readonly gap: number;
	/** The prompt it asks, for a card. */
	readonly attention: AttentionId | null;
};

/** The feed, oldest first, so the newest row sits at the bottom. With `keep`, only the items it keeps. */
export const feedRows = (
	state: TimelineState,
	keep?: (item: TimelineItem) => boolean
): FeedRow[] => {
	const rows: FeedRow[] = [];
	let before: TimelineItem["kind"] | null = null;
	for (const id of state.rows) {
		const item = state.items.get(id);
		if (!item || (keep && !keep(item))) continue;
		rows.push({
			id,
			kind: item.kind,
			gap: gapAbove(item.kind, before),
			attention: isCard(item) ? item.attention : null,
		});
		before = item.kind;
	}
	return rows;
};

/** The edit a card asks about, when it hangs under a file change. */
export const cardChanges = (state: TimelineState, card: FeedCard): readonly FileEdit[] | null => {
	const parent = card.parent !== null ? state.items.get(card.parent) : undefined;
	return parent?.kind === "file_change" ? parent.changes : null;
};

/** The ID of the feed row that shows an item, or null when the feed doesn't show it. */
export const feedRowFor = (state: TimelineState, id: string): string | null =>
	state.rows.includes(id) ? id : null;

/** Where the card for an attention event sits in the feed, or -1. */
export const feedIndexOf = (rows: readonly FeedRow[], attention: AttentionId): number =>
	rows.findIndex((row) => row.attention === attention);

/** Where you are in the feed: null at its end, or the last row there when you left it. */
export type FeedAway = { readonly last: string | null } | null;

/** Where you are after a scroll that left you at the end, or not. */
export const feedAway = (was: FeedAway, rows: readonly FeedRow[], atEnd: boolean): FeedAway =>
	atEnd ? null : (was ?? { last: rows.at(-1)?.id ?? null });

/** Whether rows came in since you left the end. */
export const rowsSince = (away: FeedAway, rows: readonly FeedRow[]): boolean =>
	away !== null && (rows.at(-1)?.id ?? null) !== away.last;
