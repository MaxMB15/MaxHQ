// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the window shows, as opposed to what the view holds: the sidebar
// mode, the selection, the agents picked for a bulk change, the open
// slide-out beside the sidebar, the Pulls page and PR beside it, and which groups and
// shelves are folded. None of it reaches the backend, apart from the focus
// the app reports.

import { createStore, type StoreApi } from "zustand/vanilla";
import type { AgentId, DeviceId, LinkId, ProjectId, PullKey, RunKey } from "../bindings";

import { pickRange, togglePick, unpickDeleted } from "./selection";

/** What the sidebar lists. */
export type SidebarMode = "projects" | "devices";

/** A shelf at the bottom of the projects view. */
export type Shelf = "working" | "snoozed" | "settled";

/** How many settled agents the Settled shelf shows before "Show more". */
export const SETTLED_SHOWN = 10;

/** How many more each "Show more" adds. */
export const SETTLED_PAGE = 25;

/** A PR the side panel shows, read from `device`. */
export type ShownPull = { readonly key: PullKey; readonly device: DeviceId };

/** A tab of the slide-out, when its lead has a run. */
export type SlideTab = "agents" | "workflow";

/**
 * The slide-out beside the sidebar: a lead's sub-agents and runs, or a run
 * no agent leads yet, which shows in run mode with no tabs.
 */
export type SlideOut = {
	/** The lead it shows, or null in run mode. */
	readonly lead: AgentId | null;
	/** The run it shows, or null for the lead's latest. */
	readonly run: RunKey | null;
	readonly tab: SlideTab;
	/** The sub-agent whose row it points out, after a card asked to show it. */
	readonly spot?: AgentId;
};

/** The window's own state. */
export type UiState = {
	/** What the sidebar lists. */
	readonly mode: SidebarMode;
	/** The agent the main area shows, if any. */
	readonly selectedAgent: AgentId | null;
	/** What slides out beside the sidebar, if anything. */
	readonly slideOut: SlideOut | null;
	/** The tab each lead's slide-out showed last. */
	readonly slideTabs: ReadonlyMap<AgentId, SlideTab>;
	/** The device the devices view shows, if one was picked. */
	readonly selectedLink: LinkId | null;
	/** The project whose Pulls page the main area shows, if any. */
	readonly pullsOf: ProjectId | null;
	/** The PR the side panel shows, if any. */
	readonly shownPull: ShownPull | null;
	/** Project groups you folded, by key. */
	readonly collapsed: ReadonlySet<string>;
	/** Project groups whose older agents you opened, by key. */
	readonly showingOlder: ReadonlySet<string>;
	/** Agents picked with Command or Shift, for a change to all of them. */
	readonly picked: ReadonlySet<AgentId>;
	/** The agent a Shift-click picks from. */
	readonly anchor: AgentId | null;
	/** Shelves you opened. */
	readonly openShelves: ReadonlySet<Shelf>;
	/** How many settled agents the Settled shelf shows. */
	readonly settledShown: number;
};

/** What changes the window's state. */
export type UiActions = {
	/** Switches the sidebar between projects and devices. */
	readonly setMode: (mode: SidebarMode) => void;
	/**
	 * Selects an agent. Its slide-out opens on the tab it showed last when
	 * it has sub-agents or a run, and closes when it has neither. A lead
	 * that hasn't shown it yet opens on `first`, Sub-agents unless it says.
	 */
	readonly selectAgent: (agent: AgentId, hasSubAgents: boolean, first?: SlideTab) => void;
	/** Selects a sub-agent without closing its lead's slide-out. */
	readonly selectSubAgent: (agent: AgentId) => void;
	/**
	 * Selects a lead and opens its slide-out on the Sub-agents tab, pointing
	 * out one sub-agent's row. The lead keeps the tab it had for next time.
	 */
	readonly showSubAgent: (lead: AgentId, subAgent: AgentId) => void;
	/** Closes the slide-out. */
	readonly closeSubAgents: () => void;
	/**
	 * Shows a run on the Workflow tab of its lead, selecting the lead, or in
	 * run mode when no agent leads it.
	 */
	readonly openRun: (run: RunKey, lead?: AgentId | null) => void;
	/** Switches the slide-out's tab, which its lead keeps for next time. */
	readonly setSlideTab: (tab: SlideTab) => void;
	/** Moves a run shown in run mode to the lead that now leads it. */
	readonly handOver: (lead: AgentId) => void;
	/** Shows a device in the devices view. */
	readonly selectLink: (link: LinkId) => void;
	/** Shows a project's Pulls page in the main area. */
	readonly openPulls: (project: ProjectId) => void;
	/** Goes back from the Pulls page to the agents. */
	readonly closePulls: () => void;
	/** Shows a PR in the side panel. */
	readonly openPull: (pull: ShownPull) => void;
	/** Closes the PR's side panel. */
	readonly closePull: () => void;
	/** Folds or unfolds a project group. */
	readonly toggleProject: (key: string) => void;
	/** Sets the folds at once, from the ones there are now. */
	readonly refold: (next: (collapsed: ReadonlySet<string>) => ReadonlySet<string>) => void;
	/** Shows or hides a project's older agents. */
	readonly toggleOlder: (key: string) => void;
	/** Picks an agent, or unpicks it when it's picked. */
	readonly togglePicked: (agent: AgentId) => void;
	/** Picks every agent from the anchor to this one, in the order given. */
	readonly pickTo: (agent: AgentId, ordered: readonly AgentId[]) => void;
	/** Unpicks every agent. */
	readonly clearPicked: () => void;
	/** Unpicks the agents a delete took and any `has` no longer finds. */
	readonly unpickDeleted: (
		deleted: ReadonlySet<AgentId>,
		has: (agent: AgentId) => boolean
	) => void;
	/** Opens or closes a shelf. */
	readonly toggleShelf: (shelf: Shelf) => void;
	/** Shows the next page of settled agents. */
	readonly showMoreSettled: () => void;
};

/** A store holding the window's state. */
export type UiStore = StoreApi<UiState & UiActions>;

const toggled = <T>(set: ReadonlySet<T>, key: T): ReadonlySet<T> => {
	const next = new Set(set);
	if (next.has(key)) next.delete(key);
	else next.add(key);
	return next;
};

const NONE: ReadonlySet<AgentId> = new Set();

/** The lead whose slide-out shows, if any. */
export const slideLead = (ui: Pick<UiState, "slideOut">): AgentId | null =>
	ui.slideOut?.lead ?? null;

/** The sub-agent whose row a lead's slide-out points out, if any. */
export const spotOf = (ui: Pick<UiState, "slideOut">, lead: AgentId): AgentId | null =>
	ui.slideOut?.lead === lead && ui.slideOut.tab === "agents" ? (ui.slideOut.spot ?? null) : null;

/** A lead's slide-out, on the tab it showed last, or on `first` the first time. */
export const slideOutOf = (
	ui: Pick<UiState, "slideTabs">,
	lead: AgentId,
	first: SlideTab = "agents"
): SlideOut => ({
	lead,
	run: null,
	tab: ui.slideTabs.get(lead) ?? first,
});

// Picking a row or a tab ends the pointing out.
const unspotted = <T extends SlideOut | null>(slideOut: T): T =>
	slideOut?.spot === undefined
		? slideOut
		: ({ lead: slideOut.lead, run: slideOut.run, tab: slideOut.tab } as T);

const withTab = (
	tabs: ReadonlyMap<AgentId, SlideTab>,
	lead: AgentId | null,
	tab: SlideTab
): ReadonlyMap<AgentId, SlideTab> => {
	if (lead === null || tabs.get(lead) === tab) return tabs;
	return new Map(tabs).set(lead, tab);
};

/** A store with nothing selected, every group open and every shelf closed. */
export const createUiStore = (initial: Partial<UiState> = {}): UiStore =>
	createStore<UiState & UiActions>()((set) => ({
		mode: "projects",
		selectedAgent: null,
		slideOut: null,
		slideTabs: new Map(),
		selectedLink: null,
		pullsOf: null,
		shownPull: null,
		collapsed: new Set(),
		showingOlder: new Set(),
		picked: NONE,
		anchor: null,
		openShelves: new Set(),
		settledShown: SETTLED_SHOWN,
		...initial,
		setMode: (mode) => set({ mode }),
		// Opening an agent ends a bulk pick and leaves the Pulls page, and a
		// Shift-click then picks from the agent you opened.
		selectAgent: (agent, hasSubAgents, first) =>
			set((s) => ({
				selectedAgent: agent,
				slideOut: hasSubAgents ? slideOutOf(s, agent, first) : null,
				pullsOf: null,
				mode: "projects",
				picked: NONE,
				anchor: agent,
			})),
		selectSubAgent: (agent) =>
			set((s) => ({ selectedAgent: agent, slideOut: unspotted(s.slideOut) })),
		showSubAgent: (lead, subAgent) =>
			set({
				selectedAgent: lead,
				slideOut: { lead, run: null, tab: "agents", spot: subAgent },
				pullsOf: null,
				mode: "projects",
				picked: NONE,
				anchor: lead,
			}),
		closeSubAgents: () => set({ slideOut: null }),
		openRun: (run, lead = null) =>
			set((s) => {
				const slideOut: SlideOut = { lead, run, tab: "workflow" };
				if (lead === null) return { slideOut };
				return {
					slideOut,
					slideTabs: withTab(s.slideTabs, lead, "workflow"),
					selectedAgent: lead,
					pullsOf: null,
					mode: "projects",
					picked: NONE,
					anchor: lead,
				};
			}),
		setSlideTab: (tab) =>
			set((s) => {
				if (!s.slideOut || s.slideOut.tab === tab) return s;
				return {
					slideOut: { ...unspotted(s.slideOut), tab },
					slideTabs: withTab(s.slideTabs, s.slideOut.lead, tab),
				};
			}),
		handOver: (lead) =>
			set((s) => {
				if (!s.slideOut || s.slideOut.lead !== null) return s;
				return {
					slideOut: { ...s.slideOut, lead },
					slideTabs: withTab(s.slideTabs, lead, "workflow"),
				};
			}),
		selectLink: (link) => set({ selectedLink: link, mode: "devices" }),
		openPulls: (project) => set({ pullsOf: project, mode: "projects" }),
		closePulls: () => set({ pullsOf: null }),
		openPull: (pull) => set({ shownPull: pull }),
		closePull: () => set({ shownPull: null }),
		toggleProject: (key) => set((s) => ({ collapsed: toggled(s.collapsed, key) })),
		refold: (next) => set((s) => ({ collapsed: next(s.collapsed) })),
		toggleOlder: (key) => set((s) => ({ showingOlder: toggled(s.showingOlder, key) })),
		togglePicked: (agent) => set((s) => togglePick(s, agent)),
		pickTo: (agent, ordered) => set((s) => pickRange(s, agent, ordered)),
		clearPicked: () => set((s) => (s.picked.size === 0 ? s : { picked: NONE })),
		unpickDeleted: (deleted, has) => set((s) => unpickDeleted(s, deleted, has)),
		toggleShelf: (shelf) => set((s) => ({ openShelves: toggled(s.openShelves, shelf) })),
		showMoreSettled: () => set((s) => ({ settledShown: s.settledShown + SETTLED_PAGE })),
	}));
