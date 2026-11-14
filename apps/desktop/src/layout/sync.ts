// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps a window's panes and its selection in step. Focusing an agent's pane
// opens its agent the way a sidebar click does, selecting an agent anywhere
// focuses or opens its pane, and closing the last pane clears the selection.
// A run's canvas leaves the selection as it is.

import type { DockviewApi } from "dockview-react";

import { openAgent } from "@/app/open-agent";
import type { Services } from "@/app/services";

import { paneAgent, paneId } from "./pane-id";
import { keepLayout, pageKept, panesOf, restoreLayout } from "./panes";

// Instances that already loaded their saved layout. React runs effects twice
// in development, and the second run mustn't load it over what the first did.
const restored = new WeakSet<DockviewApi>();

/** Wires a ready dockview instance to a window's services, and returns a call that unwires it. */
export const syncPanes = (services: Services, api: DockviewApi, label: string): (() => void) => {
	const { ui, view } = services;
	if (!restored.has(api)) {
		restored.add(api);
		// A run's canvas and a note's pane stay: runs and notes reach the
		// view once their device reports them, which can be after this. A
		// visual's page lived only in memory, so its pane goes.
		restoreLayout(api, label, (params) => {
			if (params.kind === "visual") return pageKept(params);
			return params.kind !== "agent" || view.getState().view.tables.agent.has(params.agent);
		});
	}
	const stopKeeping = keepLayout(api, label);
	const panes = panesOf(services);

	const focusing = api.onDidActivePanelChange(({ panel }) => {
		const selected = ui.getState().selectedAgent;
		if (panel) {
			const agent = paneAgent(panel.id);
			if (agent !== null && agent !== selected) openAgent(services, agent);
		} else if (api.panels.length === 0 && selected !== null) {
			ui.setState({ selectedAgent: null });
		}
	});
	const following = ui.subscribe((state, prev) => {
		const agent = state.selectedAgent;
		if (
			agent &&
			agent !== prev.selectedAgent &&
			api.activePanel?.id !== paneId({ kind: "agent", agent })
		) {
			panes.open(agent);
		}
	});
	const detach = panes.attach(api);

	// A selection made before the panes were ready gets its pane. With none,
	// the restored layout's focused pane becomes the selection, without
	// leaving the sidebar mode you're in.
	const selected = ui.getState().selectedAgent;
	if (selected) panes.open(selected);
	else {
		const agent = api.activePanel ? paneAgent(api.activePanel.id) : null;
		if (agent !== null) ui.setState({ selectedAgent: agent });
	}

	return () => {
		focusing.dispose();
		following();
		detach();
		stopKeeping();
	};
};
