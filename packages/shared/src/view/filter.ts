// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sidebar's filter. An agent stays when every word typed is in its
// title, its branch, its project's name or its device's name, ignoring
// case and accents, or when an agent shown under it stays. So a word in a
// project's name keeps all its agents. A project with no agent left hides,
// and the idle agents a group folds away show with the rest, so nothing
// that matches waits behind a fold.

import type { Agent, Project, Timestamp } from "../bindings";
import { hasWords } from "../lib/words";

import type { ViewState } from "./replica";
import {
	deviceName,
	devicesOf,
	type InboxSections,
	type ProjectGroup,
	startedRows,
} from "./select";

/** What the sidebar lists, before and after the filter. */
export type SidebarLists = {
	readonly groups: readonly ProjectGroup[];
	readonly sections: InboxSections;
	readonly hidden: readonly Project[];
};

// The texts an agent's row is found by, one to a line, so no word spans
// two. A missing one joins as an empty line.
const rowText = (view: ViewState, agent: Agent): string => {
	const ws = view.tables.workspace.get(agent.workspace);
	const project = ws?.project ? view.tables.project.get(ws.project) : undefined;
	return [agent.title, agent.branch, project?.name, ws && deviceName(view, ws.device)].join("\n");
};

/**
 * What the sidebar shows for the words: the agents that hold them, the
 * groups left with any, and the hidden projects whose names hold them.
 * With no words, the lists come back as they are.
 */
export const filterSidebar = (
	view: ViewState,
	now: Timestamp,
	lists: SidebarLists,
	words: readonly string[]
): SidebarLists => {
	if (words.length === 0) return lists;
	const holds = (agent: Agent) => hasWords(rowText(view, agent), words);
	const keep = (agent: Agent) =>
		holds(agent) || startedRows(view, agent.id, now).some((row) => holds(row.agent));
	const groups = lists.groups.flatMap((group): ProjectGroup[] => {
		const agents = [...group.agents, ...group.older].filter(keep);
		if (agents.length === 0) return [];
		return [{ ...group, devices: devicesOf(view, agents), agents, older: [] }];
	});
	const { pinned, working, snoozed, settled } = lists.sections;
	return {
		groups,
		sections: {
			pinned: pinned.filter(keep),
			working: working.filter(keep),
			snoozed: snoozed.filter(keep),
			settled: settled.filter(keep),
		},
		hidden: lists.hidden.filter((project) => hasWords(project.name, words)),
	};
};
