// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agents the projects view shows as rows, top to bottom, which is the
// order a Shift-click picks along. Rows a folded group or a closed shelf
// hides aren't in it, so a range never picks what you can't see. A closed
// shelf still shows the agent you're looking at. While
// the sidebar's filter has words, only its matches are rows, and every
// group and shelf is open unless folded since the filter began.

import type { Agent, AgentId, Timestamp } from "../bindings";
import { filterSidebar } from "../view/filter";
import type { ViewState } from "../view/replica";
import { plural } from "../status/format";
import { inboxSections, projectGroups, type SectionOptions, startedRows } from "../view/select";

import { type Shelf, slideLead, type UiState } from "./store";

/** The parts of the window's state that hide a shelf's rows. */
export type ShelfShown = Pick<
	UiState,
	"openShelves" | "settledShown" | "selectedAgent" | "slideOut"
>;

/** The parts of the window's state that hide rows. */
export type RowsShown = ShelfShown & Pick<UiState, "collapsed" | "showingOlder">;

/**
 * The rows a shelf shows: every one while it's open, though only a page of
 * the settled ones, and only the one you're looking at while it's closed.
 */
export const shelfRows = (shelf: Shelf, agents: readonly Agent[], ui: ShelfShown): Agent[] => {
	const looking = agents.find(
		(agent) => agent.id === ui.selectedAgent || agent.id === slideLead(ui)
	);
	if (!ui.openShelves.has(shelf)) return looking ? [looking] : [];
	if (shelf !== "settled") return [...agents];
	const page = agents.slice(0, ui.settledShown);
	return looking && !page.includes(looking) ? [...page, looking] : page;
};

/**
 * The key a project group's or a shelf's fold has while the sidebar's
 * filter has words.
 */
export const foldKey = (kind: "project" | "shelf", key: string): string => `${kind}:${key}`;

/** What the sidebar's filter holds: its words, and what was folded since. */
export type RowsFilter = {
	readonly words: readonly string[];
	readonly folded: ReadonlySet<string>;
};

const SHELVES: readonly Shelf[] = ["working", "snoozed", "settled"];

/**
 * The agents shown as rows, in order: the pinned ones, each open project
 * group with its older agents when they're shown, then each open shelf,
 * with the agents each one started right under it.
 */
export const shownRows = (
	view: ViewState,
	now: Timestamp,
	options: SectionOptions,
	ui: RowsShown,
	filter: RowsFilter = { words: [], folded: new Set() }
): AgentId[] => {
	const lists = filterSidebar(
		view,
		now,
		{
			groups: projectGroups(view, now, options),
			sections: inboxSections(view, now, options),
			hidden: [],
		},
		filter.words
	);
	const filtering = filter.words.length > 0;
	const open = (key: string, left: boolean) => (filtering ? !filter.folded.has(key) : left);
	const groups = lists.groups.flatMap((group) =>
		open(foldKey("project", group.key), !ui.collapsed.has(group.key))
			? [...group.agents, ...(ui.showingOlder.has(group.key) ? group.older : [])]
			: []
	);
	const shelves = {
		...ui,
		openShelves: new Set(
			SHELVES.filter((shelf) => open(foldKey("shelf", shelf), ui.openShelves.has(shelf)))
		),
	};
	const { sections } = lists;
	return [
		...sections.pinned,
		...groups,
		...SHELVES.flatMap((shelf) => shelfRows(shelf, sections[shelf], shelves)),
	].flatMap((agent) => [
		agent.id,
		...startedRows(view, agent.id, now).map((row) => row.agent.id),
	]);
};

/**
 * The agents a drag from this row carries: every picked agent when the row
 * is one of them, the pressed one first, and just this one otherwise.
 */
export const draggedAgents = (
	view: ViewState,
	agent: Agent,
	picked: ReadonlySet<AgentId>
): Agent[] => {
	if (!picked.has(agent.id)) return [agent];
	const others = [...picked].flatMap((id) => {
		const other = view.tables.agent.get(id);
		return other && id !== agent.id ? [other] : [];
	});
	return [agent, ...others];
};

/** What a drop of these agents adds: the one agent's title, or how many. */
export const draggedName = (agents: readonly Agent[]): string =>
	agents.length === 1 ? agents[0]!.title : plural(agents.length, "agent");
