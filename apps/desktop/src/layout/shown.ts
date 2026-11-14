// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a window's panes show, for the backend's notifications (decision
// C-8). A prompt alerts unless its agent's pane has focus. A finished turn,
// a usage limit and a resume alert only when no pane shows the agent. So
// each window reports the pane with focus, if it has one, and every pane on
// screen: the active pane of each group, docked or floating.

import type { DockviewApi } from "dockview-react";

import { markSeenIfLooking } from "@/app/open-agent";
import type { Services } from "@/app/services";
import type { AgentId, Shown } from "@maxhq/shared/bindings";

import { panesOf } from "./panes";

/** The parts of a dockview group this reads. A live group has them. */
export type GroupLike = {
	readonly activePanel:
		{ readonly params: Readonly<Record<string, unknown>> | undefined } | undefined;
	readonly api: {
		readonly isVisible: boolean;
		readonly location: { readonly type: string };
	};
};

/** The parts of a dockview instance this reads. A live instance has them. */
export type DockLike = {
	readonly groups: readonly GroupLike[];
	readonly activeGroup: GroupLike | undefined;
};

/** Whether the window this runs in has focus, and whether it's on screen. */
export type WindowState = {
	readonly focused: boolean;
	readonly visible: boolean;
};

// The agent in a group's active pane, if the group shows. A group hides
// behind a maximized one. Dockview's own pop-outs open a browser window this
// one can't see into, and the app doesn't make them.
const agentIn = (group: GroupLike | undefined): AgentId | null => {
	if (!group?.api.isVisible || group.api.location.type === "popout") return null;
	const agent = group.activePanel?.params?.agent;
	return typeof agent === "string" ? agent : null;
};

/** The agents whose panes show: the active pane of each docked or floating group on screen. */
export const visibleAgents = (dock: DockLike): AgentId[] => {
	const agents = new Set<AgentId>();
	for (const group of dock.groups) {
		const agent = agentIn(group);
		if (agent !== null) agents.add(agent);
	}
	return [...agents].sort();
};

/**
 * What a window's panes show. With no panes, such as while they hide under
 * the devices view, or with the window off screen, nothing shows. Only a
 * window with focus has a focused pane.
 */
export const shownIn = (dock: DockLike | null, window: WindowState): Shown => {
	if (!dock || !window.visible) return { focused: null, visible: [] };
	return {
		focused: window.focused ? agentIn(dock.activeGroup) : null,
		visible: visibleAgents(dock),
	};
};

/** The state of the window this runs in, from the document. */
export const windowState = (): WindowState => ({
	focused: document.hasFocus(),
	visible: document.visibilityState !== "hidden",
});

// Every dockview event after which a different pane may show.
const watchDock = (api: DockviewApi, changed: () => void): (() => void) => {
	const listening = [
		api.onDidActivePanelChange(changed),
		api.onDidActiveGroupChange(changed),
		api.onDidAddPanel(changed),
		api.onDidRemovePanel(changed),
		api.onDidMovePanel(changed),
		api.onDidAddGroup(changed),
		api.onDidRemoveGroup(changed),
		api.onDidMaximizedGroupChange(changed),
		api.onDidLayoutFromJSON(changed),
		api.onDidLayoutChange(changed),
	];
	return () => {
		for (const disposable of listening) disposable.dispose();
	};
};

/**
 * Tells the backend what this window's panes show each time that may have
 * changed, and marks the agent in the focused pane seen when it's done.
 * Returns a call that stops.
 */
export const watchShown = (services: Services): (() => void) => {
	const { source, ui, view } = services;
	const panes = panesOf(services);
	let stopped = false;
	let queued = false;
	let sent: string | null = null;

	// The panes stay mounted under the devices view, but you can't see them.
	const current = (): Shown =>
		shownIn(ui.getState().mode === "projects" ? panes.api() : null, windowState());

	const report = () => {
		queued = false;
		if (stopped) return;
		const shown = current();
		markSeenIfLooking(services, shown.focused);
		const key = JSON.stringify(shown);
		if (key === sent) return;
		sent = key;
		source.setShown(shown).catch((error: unknown) => {
			console.warn("Couldn't tell the app what the panes show", error);
		});
	};
	// One change fires several dockview events, so they make one report
	// once the change is done.
	const changed = () => {
		if (queued) return;
		queued = true;
		queueMicrotask(report);
	};

	let stopDock: (() => void) | undefined;
	const stopPanes = panes.watch((api) => {
		stopDock?.();
		stopDock = api ? watchDock(api, changed) : undefined;
		changed();
	});
	const stopUi = ui.subscribe((state, prev) => {
		if (state.mode !== prev.mode) changed();
	});
	const stopView = view.subscribe(() => markSeenIfLooking(services, current().focused));
	window.addEventListener("focus", changed);
	window.addEventListener("blur", changed);
	document.addEventListener("visibilitychange", changed);

	return () => {
		stopped = true;
		stopPanes();
		stopDock?.();
		stopUi();
		stopView();
		window.removeEventListener("focus", changed);
		window.removeEventListener("blur", changed);
		document.removeEventListener("visibilitychange", changed);
	};
};
