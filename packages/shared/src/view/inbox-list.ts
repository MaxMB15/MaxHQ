// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox as one list, for a screen with room for a single column, laid
// out as the desktop's projects view: every agent waiting on you, the
// longest waiting first, then the pinned ones, then one group per project,
// then the busy ones while the Working shelf is on, then the snoozed and
// settled ones. As on the desktop, an agent waiting on you also keeps its
// row in its group, and an agent another one started sits under that one.
// The filter's words narrow it as they narrow the desktop's sidebar, and
// leave the agents waiting on you, which sit above the field.

import type { Agent, Project, ProjectId, Timestamp } from "../bindings";
import { keepsInboxes } from "../settings/inbox";

import { filterSidebar } from "./filter";
import { inboxMenu, type InboxMenu } from "./inbox-actions";
import type { ViewState } from "./replica";
import {
	hiddenProjectList,
	inboxSections,
	type NeedsYouItem,
	needsYou,
	projectGroups,
	startedRows,
} from "./select";

/** One row: an agent, and how many levels under its caller it sits. */
export type InboxRow = { readonly agent: Agent; readonly level: number };

/** One project's rows. */
export type InboxGroup = {
	/** The project's ID, or "none" for agents without one. */
	readonly key: string;
	/** The project, or null for agents without one. */
	readonly project: ProjectId | null;
	readonly name: string;
	/** Whether you hid the project, so it shows only while an agent calls it back. */
	readonly hidden: boolean;
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
	/** Busy, the newest first, while the Working shelf is on. */
	readonly working: readonly InboxRow[];
	/** Snoozed, the soonest to wake first. */
	readonly snoozed: readonly Agent[];
	/** Settled, the most recently settled first. */
	readonly settled: readonly Agent[];
	/** The projects you hid, by name, for the Hidden projects entry. */
	readonly hidden: readonly Project[];
};

/** How the list is laid out and narrowed. */
export type InboxListOptions = {
	/** Whether busy agents move from their groups to the Working shelf. */
	readonly workingShelf?: boolean;
	/** The filter's words, as `searchWords` splits them. None shows everything. */
	readonly words?: readonly string[];
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
 * gets a group with no rows, so its Notes entry shows, until a filter
 * leaves it no agent.
 */
export const inboxList = (
	view: ViewState,
	now: Timestamp,
	options: InboxListOptions = {}
): InboxList => {
	const shelf = { workingShelf: options.workingShelf ?? false };
	const lists = filterSidebar(
		view,
		now,
		{
			groups: projectGroups(view, now, shelf),
			sections: inboxSections(view, now, shelf),
			hidden: hiddenProjectList(view),
		},
		options.words ?? []
	);
	const { pinned, working, snoozed, settled } = lists.sections;
	return {
		waiting: needsYou(view),
		pinned: withStarted(view, pinned, now),
		groups: lists.groups.map((group) => ({
			key: group.key,
			project: group.project?.id ?? null,
			hidden: group.hidden,
			name: group.name,
			rows: withStarted(view, group.agents, now),
			older: withStarted(view, group.older, now),
		})),
		working: withStarted(view, working, now),
		snoozed,
		settled,
		hidden: lists.hidden,
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
