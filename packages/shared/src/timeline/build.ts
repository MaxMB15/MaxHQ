// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Builds timeline items for tests and sample data. Each takes the body and
// the fields that matter, and fills in the rest.

import type { ItemBody, TimelineItem } from "../bindings";

/** The fields every item has, whatever its body. */
export type ItemHead = Pick<
	TimelineItem,
	"id" | "agent" | "turn" | "parent" | "ordinal" | "status" | "started_at" | "completed_at"
>;

/** An item for agent "a1", completed, at ordinal 0 unless told otherwise. */
export const timelineItem = (
	body: ItemBody,
	over: Partial<ItemHead> & { readonly id: string }
): TimelineItem => ({
	agent: "a1",
	turn: null,
	parent: null,
	ordinal: 0,
	status: "completed",
	started_at: null,
	completed_at: null,
	...over,
	...body,
});

const KINDS: Record<ItemBody["kind"], true> = {
	user_message: true,
	assistant_message: true,
	reasoning: true,
	command: true,
	file_change: true,
	file_read: true,
	search: true,
	web: true,
	tool_call: true,
	subagent: true,
	plan: true,
	todos: true,
	approval: true,
	question: true,
	error: true,
	limit_wait: true,
	compaction: true,
	interrupted: true,
	turn_summary: true,
	notice: true,
	handoff: true,
	agent_attachments: true,
	agent_opened_file: true,
	agent_visual: true,
	user_run: true,
	user_session: true,
	unknown: true,
};

/** Every kind of item, so tests can check each one is covered. */
export const ITEM_KINDS = Object.keys(KINDS) as ItemBody["kind"][];
