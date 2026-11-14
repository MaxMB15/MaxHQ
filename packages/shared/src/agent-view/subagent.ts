// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a sub-agent's card says about its work: how many tool calls it has
// made, the step it's on, and the line under its description.

import { t } from "../i18n";

import { useStore } from "zustand";

import type { TimelineItem, Timestamp } from "../bindings";
import { formatDuration, plural } from "../status/format";
import type { TimelineState } from "../timeline/apply";
import type { TimelineStore } from "../timeline/store";
import { isOpen, TOOL_KINDS, tookOf, toolTarget } from "./format";

/** Which items in a timeline are the sub-agent's own. */
export type Mine = (item: TimelineItem) => boolean;

/** Every item, for a timeline of the sub-agent's own. */
export const everything: Mine = () => true;

/** What a sub-agent has done so far. */
export type Work = {
	/** Its tool calls, or null before its timeline has any. */
	readonly tools: number | null;
	/** Set when older items haven't loaded, so there were more calls than counted. */
	readonly partial: boolean;
	/** What the step it's on acts on, while it runs one. */
	readonly now: string | null;
};

const NO_WORK: Work = { tools: null, partial: false, now: null };

const countTools = (state: TimelineState, mine: Mine): number => {
	let count = 0;
	for (const item of state.items.values()) if (TOOL_KINDS.has(item.kind) && mine(item)) count++;
	return count;
};

// The latest tool step that hasn't ended.
const currentTool = (state: TimelineState, mine: Mine): string | null => {
	for (let i = state.order.length - 1; i >= 0; i--) {
		const item = state.items.get(state.order[i]!);
		if (item && TOOL_KINDS.has(item.kind) && isOpen(item.status) && mine(item))
			return toolTarget(item);
	}
	return null;
};

/**
 * A sub-agent's work, from `store`. Steps nested under a card come after it,
 * so only a timeline of the sub-agent's own (`own`) can have calls that
 * haven't loaded.
 */
export const useWork = (store: TimelineStore, mine: Mine, running: boolean, own: boolean): Work => {
	const tools = useStore(store, (s) => countTools(s, mine));
	const partial = useStore(store, (s) => own && s.hasOlder);
	const now = useStore(store, (s) => (running ? currentTool(s, mine) : null));
	return tools > 0 ? { tools, partial, now } : NO_WORK;
};

const callsOf = (work: Work): string | null =>
	work.tools !== null
		? `${work.partial ? t("at least ") : ""}${plural(work.tools, "tool call")}`
		: null;

/** The line under a running sub-agent: these words, then a clock from `since` when it's set. */
export const runningLine = (
	startedAt: Timestamp | null,
	work: Work
): { readonly words: string; readonly since: Timestamp | null } => {
	const calls = callsOf(work);
	if (startedAt === null) {
		return {
			words:
				calls !== null
					? t("Sub-agent, {{calls}}", { calls: calls })
					: t("Sub-agent, running"),
			since: null,
		};
	}
	return {
		words:
			calls !== null
				? t("Sub-agent, {{calls}}, ", { calls: calls })
				: t("Sub-agent, running for "),
		since: startedAt,
	};
};

/** The line under a sub-agent that has ended. */
export const endedLine = (
	item: Pick<TimelineItem, "status" | "started_at" | "completed_at">,
	work: Work
): string => {
	const calls = callsOf(work);
	const after = calls !== null ? t(" after {{value1}}", { value1: calls }) : "";
	switch (item.status) {
		case "failed":
			return t("Sub-agent, failed{{after}}", { after: after });
		case "cancelled":
		case "interrupted":
		case "declined":
			return t("Sub-agent, stopped{{after}}", { after: after });
		default: {
			const took = tookOf(item);
			if (calls === null && took === null) return t("Sub-agent, done");
			return [
				t("Sub-agent"),
				calls,
				took !== null ? t("took {{value1}}", { value1: formatDuration(took) }) : null,
			]
				.filter(Boolean)
				.join(", ");
		}
	}
};
