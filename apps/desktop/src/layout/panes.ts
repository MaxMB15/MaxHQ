// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The panes in a window's main area, one controller per window. Every way of
// opening an agent goes through it: a sidebar click, a notification or a new
// agent focuses the agent's pane when it has one, and adds a tab to the
// focused group when it doesn't. A workflow run's canvas and a project
// note open the same way, and so does a file from an agent's device, a
// visual and a terminal out of an agent's drawer.
// The dockview instance attaches once it's ready, and an open that comes
// before that waits for it. It also keeps the panes you close, so reopening
// one puts it back where it was.

import { t } from "@maxhq/shared/i18n";

import type { AddPanelPositionOptions, DockviewApi } from "dockview-react";

import type { Services } from "@/app/services";
import type { AgentId, NoteId, ProjectId, RunKey, TerminalId } from "@maxhq/shared/bindings";
import { finishedTitle } from "@maxhq/shared/agent-view/finished";
import { fileName } from "@maxhq/shared/agent-view/viewer-files";
import { tabTitle } from "@maxhq/shared/terminal/drawer";
import { terminalById } from "@maxhq/shared/terminal/select";
import { noteSummary } from "@maxhq/shared/view/notes";
import type { VisualTransfer } from "@maxhq/shared/visuals/transfer";
import { runAt } from "@maxhq/shared/workflow/select";
import { revealLine } from "@/viewer/reveal";
import { keepVisualPage, visualPage } from "@/visuals/tab-pages";

import { type ClosedPane, watchClosing } from "./closed";
import { focusPane } from "./commands";
import { type PaneParams, paneId, paneParams } from "./pane-id";
import { clearLayout, loadLayout, saveLayout } from "./storage";

/** The component every agent pane and its tab render with. */
export const AGENT_PANE = "agent";
/** The component every workflow run's canvas and its tab render with. */
export const WORKFLOW_PANE = "workflow";
/** The component every project note's pane and its tab render with. */
export const NOTE_PANE = "note";

/** The component every file pane and its tab render with. */
export const FILE_PANE = "file";

/** The component every visual's pane and its tab render with. */
export const VISUAL_PANE = "visual";

/** The component every terminal's pane and its tab render with. */
export const TERMINAL_PANE = "terminal";

const COMPONENT = {
	agent: AGENT_PANE,
	workflow: WORKFLOW_PANE,
	note: NOTE_PANE,
	file: FILE_PANE,
	visual: VISUAL_PANE,
	terminal: TERMINAL_PANE,
} as const;

export type { PaneParams } from "./pane-id";

/** A side of a group, or its middle, which takes a tab. */
export type DropSide = "center" | "left" | "right" | "top" | "bottom";

/** Where a dragged agent lands: on a group, or on the whole grid when `group` is null. */
export type DropSpot = { readonly group: string | null; readonly side: DropSide };

/** How to open an agent. */
export type OpenOptions = {
	/** Opens it in a new group beside the focused one instead of as a tab in it. */
	readonly split?: boolean;
	/** Puts its pane at this spot, moving the pane there when it has one. */
	readonly at?: DropSpot;
};

/** How to open a file. */
export type FileOpenOptions = OpenOptions & {
	/**
	 * Scrolls the file to this line and marks it, in a new pane or one
	 * already open. It isn't part of the pane's ID, so one file keeps one
	 * pane.
	 */
	readonly line?: number;
};

/** A window's panes. */
export type Panes = {
	/** Hands over a ready dockview instance and returns a call that takes it back. */
	readonly attach: (api: DockviewApi) => () => void;
	/** Focuses the agent's pane, or opens one. */
	readonly open: (agent: AgentId, options?: OpenOptions) => void;
	/** Focuses the run's canvas, or opens one. */
	readonly openRun: (run: RunKey, options?: OpenOptions) => void;
	/** Focuses the note's pane, or opens one. */
	readonly openNote: (project: ProjectId, note: NoteId, options?: OpenOptions) => void;
	/** Focuses the pane that shows a file from the agent's device, or opens one. */
	readonly openFile: (agent: AgentId, path: string, options?: FileOpenOptions) => void;
	/** Focuses the pane that shows this visual, or opens one. */
	readonly openVisual: (visual: VisualTransfer, options?: OpenOptions) => void;
	/** Focuses the pane that shows a terminal out of the agent's drawer, or opens one. */
	readonly openTerminal: (agent: AgentId, terminal: TerminalId, options?: OpenOptions) => void;
	/** Puts the last pane you closed back where it was. Returns whether there was one. */
	readonly reopen: () => boolean;
	/** Whether there's a closed pane to put back. */
	readonly canReopen: () => boolean;
	/** What the closed panes reopening can still put back showed, the last closed first. */
	readonly recentlyClosed: () => readonly PaneParams[];
	/** The attached instance, if any. */
	readonly api: () => DockviewApi | null;
	/** Calls `listener` with the attached instance now and each time it changes, until the returned call. */
	readonly watch: (listener: (api: DockviewApi | null) => void) => () => void;
};

// Where a new pane goes: a tab in the focused group, or a new group to its
// right. A floating group takes no neighbors, so a split from one lands at
// the right of the grid instead.
const placeFor = (api: DockviewApi, split: boolean): AddPanelPositionOptions | undefined => {
	const group = api.activeGroup;
	if (!group) return undefined;
	if (!split) return { referenceGroup: group, direction: "within" };
	if (group.api.location.type === "floating") return { direction: "right" };
	return { referenceGroup: group, direction: "right" };
};

const paneFor = (params: PaneParams, title: string) => ({
	id: paneId(params),
	component: COMPONENT[params.kind],
	tabComponent: COMPONENT[params.kind],
	params,
	title,
	// A file or visual pane holds a frame, which reloads whenever it moves
	// in the page, so it stays where it is and dockview lays it over its group.
	...(params.kind === "file" || params.kind === "visual" ? { renderer: "always" as const } : {}),
});

/**
 * Whether a pane's page is here: a visual's page lives only in this
 * window's memory, so after a restart its pane has nothing to show. Panes
 * of every other kind say yes.
 */
export const pageKept = (params: PaneParams): boolean =>
	params.kind !== "visual" || visualPage(params.key) !== undefined;

const DIRECTION = {
	center: "within",
	left: "left",
	right: "right",
	top: "above",
	bottom: "below",
} as const;

// Puts a pane where it was dropped: a tab in the middle of a group, or a
// new group at one of its sides or along a side of the grid. The middle of
// a grid with no groups takes a group too. A pane that's already open moves
// there, unless it's alone in the group it lands beside, which leaves
// nothing to move.
const drop = (api: DockviewApi, params: PaneParams, title: string, spot: DropSpot): void => {
	const { side } = spot;
	const group = spot.group === null ? null : api.groups.find((g) => g.id === spot.group);
	if (group === undefined) return show(api, params, title, {});
	if (side !== "center" && api.hasMaximizedGroup()) api.exitMaximizedGroup();
	const along = side === "center" ? "right" : DIRECTION[side];
	const existing = api.getPanel(paneId(params));
	if (!existing) {
		api.addPanel({
			...paneFor(params, title),
			position: group
				? { referenceGroup: group, direction: DIRECTION[side] }
				: { direction: along },
		});
	} else if (!group) {
		existing.api.moveTo({ group: api.addGroup({ direction: along }) });
	} else if (group === existing.group && (side === "center" || group.panels.length === 1)) {
		focusPane(existing);
	} else {
		existing.api.moveTo({ group, position: side });
	}
};

const show = (
	api: DockviewApi,
	params: PaneParams,
	title: string,
	{ split = false, at }: OpenOptions
): void => {
	if (at) return drop(api, params, title, at);
	const existing = api.getPanel(paneId(params));
	if (existing) {
		if (api.activePanel === existing) return;
		if (api.hasMaximizedGroup() && !existing.api.isMaximized()) api.exitMaximizedGroup();
		focusPane(existing);
		return;
	}
	if (split && api.hasMaximizedGroup()) api.exitMaximizedGroup();
	api.addPanel<PaneParams>({ ...paneFor(params, title), position: placeFor(api, split) });
};

// Puts a closed pane back: in its own group when that's still open, in a
// floating group in the same box, split from the group it sat beside, or
// as a tab in the focused group when none of those are left. `group` finds
// what a group became, since a group that closes comes back with a new ID.
const putBack = (
	api: DockviewApi,
	pane: ClosedPane,
	title: string,
	group: (id: string) => string | undefined
): void => {
	if (api.hasMaximizedGroup()) api.exitMaximizedGroup();
	const options = paneFor(pane.params, title);
	const home = group(pane.group);
	const beside = pane.beside && (pane.beside.group === null ? null : group(pane.beside.group));
	if (home) {
		const index = Math.min(pane.index, api.getGroup(home)?.panels.length ?? 0);
		api.addPanel({
			...options,
			position: { referenceGroup: home, direction: "within", index },
		});
	} else if (pane.floating) {
		api.addPanel({ ...options, floating: pane.floating });
	} else if (pane.beside && beside !== undefined) {
		const { side, size } = pane.beside;
		const across = side === "left" || side === "right";
		api.addPanel({
			...options,
			position: beside ? { referenceGroup: beside, direction: side } : { direction: side },
			...(across ? { initialWidth: size } : { initialHeight: size }),
		});
	} else {
		api.addPanel({ ...options, position: placeFor(api, false) });
	}
};

// How many closed panes reopening can go back through.
const KEPT_CLOSED = 20;

/**
 * A controller that names new panes with `title`. Reopening skips the panes
 * whose agent or run is gone, which `exists` tells.
 */
export const createPanes = (
	title: (params: PaneParams) => string,
	exists: (params: PaneParams) => boolean = () => true
): Panes => {
	let current: DockviewApi | null = null;
	let pending: { params: PaneParams; options: OpenOptions } | null = null;
	const listeners = new Set<(api: DockviewApi | null) => void>();
	const changed = () => {
		for (const listener of [...listeners]) listener(current);
	};
	const closed: ClosedPane[] = [];
	const renamed = new Map<string, string>();
	const reopenable = (api: DockviewApi, pane: ClosedPane) =>
		exists(pane.params) && !api.getPanel(paneId(pane.params));
	const groupNow = (api: DockviewApi, id: string): string | undefined => {
		const seen = new Set<string>();
		let at = id;
		while (!api.getGroup(at) && renamed.has(at) && !seen.has(at)) {
			seen.add(at);
			at = renamed.get(at)!;
		}
		return api.getGroup(at) ? at : undefined;
	};
	const open = (params: PaneParams, options: OpenOptions = {}) => {
		if (current) show(current, params, title(params), options);
		else pending = { params, options };
	};

	return {
		attach: (api) => {
			current = api;
			const stopWatching = watchClosing(api, (pane) => {
				closed.push(pane);
				if (closed.length > KEPT_CLOSED) closed.shift();
			});
			if (pending) {
				const { params, options } = pending;
				pending = null;
				show(api, params, title(params), options);
			}
			changed();
			return () => {
				stopWatching();
				if (current !== api) return;
				current = null;
				changed();
			};
		},
		open: (agent, options) => open({ kind: "agent", agent }, options),
		openRun: (run, options) => open({ kind: "workflow", run }, options),
		openNote: (project, note, options) => open({ kind: "note", project, note }, options),
		openFile: (agent, path, { line, ...options } = {}) => {
			revealLine(agent, path, line);
			open({ kind: "file", agent, path }, options);
		},
		openVisual: (visual, options) =>
			open({ kind: "visual", key: keepVisualPage(visual), title: visual.title }, options),
		openTerminal: (agent, terminal, options) =>
			open({ kind: "terminal", agent, terminal }, options),
		reopen: () => {
			const api = current;
			if (!api) return false;
			for (let pane = closed.pop(); pane; pane = closed.pop()) {
				if (!reopenable(api, pane)) continue;
				putBack(api, pane, title(pane.params), (id) => groupNow(api, id));
				const landed = api.getPanel(paneId(pane.params))?.group.id;
				if (landed && landed !== pane.group) renamed.set(pane.group, landed);
				return true;
			}
			return false;
		},
		canReopen: () => {
			const api = current;
			return api !== null && closed.some((pane) => reopenable(api, pane));
		},
		recentlyClosed: () => {
			const api = current;
			if (!api) return [];
			const seen = new Set<string>();
			return [...closed].reverse().flatMap((pane) => {
				const id = paneId(pane.params);
				if (seen.has(id) || !reopenable(api, pane)) return [];
				seen.add(id);
				return [pane.params];
			});
		},
		api: () => current,
		watch: (listener) => {
			listeners.add(listener);
			listener(current);
			return () => {
				listeners.delete(listener);
			};
		},
	};
};

const byServices = new WeakMap<Services, Panes>();

/** The panes of the window these services run. */
export const panesOf = (services: Services): Panes => {
	let panes = byServices.get(services);
	if (!panes) {
		const view = () => services.view.getState().view;
		panes = createPanes(
			(params) => {
				if (params.kind === "agent")
					return (
						view().tables.agent.get(params.agent)?.title ??
						finishedTitle(params.agent) ??
						t("Agent")
					);
				if (params.kind === "note")
					return noteSummary(view(), params.project, params.note)?.title ?? t("Note");
				if (params.kind === "file") return fileName(params.path);
				if (params.kind === "visual") return params.title || t("Visual");
				if (params.kind === "terminal") {
					const terminal = terminalById(view(), params.terminal);
					return terminal ? tabTitle(terminal) : t("Terminal");
				}
				return runAt(view(), params.run)?.label ?? t("Workflow");
			},
			(params) => {
				if (params.kind === "agent") return view().tables.agent.has(params.agent);
				if (params.kind === "note") {
					const note = noteSummary(view(), params.project, params.note);
					return note !== undefined && !note.deleted;
				}
				if (params.kind === "file") return view().tables.agent.has(params.agent);
				if (params.kind === "visual") return pageKept(params);
				if (params.kind === "terminal")
					return terminalById(view(), params.terminal) !== undefined;
				return runAt(view(), params.run) !== undefined;
			}
		);
		byServices.set(services, panes);
	}
	return panes;
};

const KNOWN: ReadonlySet<string> = new Set(Object.values(COMPONENT));

/**
 * Loads a window's saved layout into an empty instance, keeping only the
 * panes `exists` keeps. A layout that won't load leaves the instance empty
 * and is forgotten.
 */
export const restoreLayout = (
	api: DockviewApi,
	label: string,
	exists: (params: PaneParams) => boolean
): void => {
	const saved = loadLayout(label);
	if (!saved) return;
	if (Object.values(saved.panels).some((p) => !KNOWN.has(p.contentComponent ?? ""))) {
		clearLayout(label);
		return;
	}
	try {
		api.fromJSON(saved);
	} catch (error) {
		console.warn("Couldn't restore the pane layout", error);
		api.clear();
		clearLayout(label);
		return;
	}
	for (const panel of [...api.panels]) {
		if (!exists(paneParams(panel.id))) api.removePanel(panel);
	}
};

/** Saves a window's layout a moment after each change, and once more when stopped. */
export const keepLayout = (api: DockviewApi, label: string, delay = 300): (() => void) => {
	let timer: ReturnType<typeof setTimeout> | undefined;
	const save = () => {
		clearTimeout(timer);
		timer = undefined;
		try {
			saveLayout(label, api.toJSON());
		} catch (error) {
			console.warn("Couldn't save the pane layout", error);
		}
	};
	const watching = api.onDidLayoutChange(() => {
		clearTimeout(timer);
		timer = setTimeout(save, delay);
	});
	return () => {
		watching.dispose();
		save();
	};
};
