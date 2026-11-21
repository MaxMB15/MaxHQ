// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the dispatcher's conversation shows on every client: what was said,
// what it asks, and what went wrong. Its reading and tool calls stay in the
// agent's own view. Also the short hints around it.

import type { Agent, TimelineItem } from "../bindings";

/** The kinds of item the conversation shows. */
export const CONVERSATION_KINDS: ReadonlySet<TimelineItem["kind"]> = new Set<TimelineItem["kind"]>([
	"user_message",
	"assistant_message",
	"question",
	"approval",
	"error",
	"notice",
	"limit_wait",
	"dispatcher_action",
]);

/** Whether the conversation shows an item. */
export const inConversation = (item: TimelineItem): boolean => CONVERSATION_KINDS.has(item.kind);

/** The items the conversation shows, oldest first. */
export const conversationItems = (
	rows: readonly string[],
	items: ReadonlyMap<string, TimelineItem>
): TimelineItem[] =>
	rows
		.map((id) => items.get(id))
		.filter((i): i is TimelineItem => i !== undefined && inConversation(i));

const WORKING = new Set<Agent["activity"]["state"]>(["starting", "busy", "busy_in_subagents"]);

/** Whether the dispatcher is working on what you asked. */
export const isWorking = (agent: Agent): boolean => WORKING.has(agent.activity.state);

/** Under the conversation while the dispatcher works. */
export const WORKING_HINT = "The dispatcher is working";

/** In place of a conversation: one on its way, or what your next request does. */
export const noConversationHint = (busy: boolean): string =>
	busy ? "Starting the dispatcher" : "What you ask next starts a new conversation.";

/** Next to a plan's Confirm. */
export const CONFIRM_HINT = "Nothing runs until you confirm.";
