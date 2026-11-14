// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox as one list, for a screen with room for a single column, laid
// out as the desktop's projects view: every agent waiting on you, the
// longest waiting first, then the pinned ones, then one group per project,
// then the snoozed and settled ones. As on the desktop, an agent waiting on
// you also keeps its row in its group, and an agent another one started
// sits under that one.

import type { Agent, ProjectId, Timestamp } from "../bindings";
import { keepsInboxes } from "../settings/inbox";

import { inboxMenu, type InboxMenu } from "./inbox-actions";
import type { ViewState } from "./replica";
import { inboxSections, type NeedsYouItem, needsYou, projectGroups, startedRows } from "./select";

/** One row: an agent, and how many levels under its caller it sits. */
export type InboxRow = { readonly agent: Agent; readonly level: number };

/** One project's rows. */
export type InboxGroup = {
	/** The project's ID, or "none" for agents without one. */
	readonly key: string;
	/** The project, or null for agents without one. */
	readonly project: ProjectId | null;
	readonly name: string;
	/** Sorted, each followed by the agents it started. */
	readonly rows: readonly InboxRow[];
	/** Idle agents over a day old, behind an "older" row. */
	readonly older: readonly InboxRow[];
};

/** The inbox's parts, top to bottom. */
export type InboxList = {
	/** Waiting on you, the longest waiting first. */
	readonly waiting: readonly NeedsYouItem[];
	/** Pinned, in their order. */
	readonly pinned: readonly InboxRow[];
	/** Projects by name, the agents without one last. */
	readonly groups: readonly InboxGroup[];
	/** Snoozed, the soonest to wake first. */
	readonly snoozed: readonly Agent[];
	/** Settled, the most recently settled first. */
	readonly settled: readonly Agent[];
};

/** Each agent followed by the agents it started, as the desktop's rows run. */
export const withStarted = (
	view: ViewState,
	agents: readonly Agent[],
	now: Timestamp
): InboxRow[] =>
	agents.flatMap((agent) => [{ agent, level: 0 }, ...startedRows(view, agent.id, now)]);

/**
 * The whole inbox, each part sorted. A project there only for its notes
 * gets a group with no rows, so its Notes entry shows.
 */
export const inboxList = (view: ViewState, now: Timestamp): InboxList => {
	const sections = inboxSections(view, now);
	return {
		waiting: needsYou(view),
		pinned: withStarted(view, sections.pinned, now),
		groups: projectGroups(view, now).map((group) => ({
			key: group.key,
			project: group.project?.id ?? null,
			name: group.name,
			rows: withStarted(view, group.agents, now),
			older: withStarted(view, group.older, now),
		})),
		snoozed: sections.snoozed,
		settled: sections.settled,
	};
};

/**
 * What a swipe on the agent's row offers: the inbox menu for it alone, or
 * null when its device's `maxd` doesn't keep inboxes, so there's nothing to
 * change. A sub-agent goes where its lead goes, so it gets none either.
 */
export const swipeMenu = (view: ViewState, agent: Agent, now: Timestamp): InboxMenu | null => {
	if (agent.parent !== null) return null;
	const workspace = view.tables.workspace.get(agent.workspace);
	if (!workspace || !keepsInboxes(view, workspace.device)) return null;
	return inboxMenu(view, [agent], now);
};
