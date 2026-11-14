// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether the window's new-agent form is open, and for which project. The
// sidebar and the shortcut open it, and the form closes it.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import type { ProjectId } from "@maxhq/shared/bindings";

/** The form's state. */
export type NewAgentState = {
	readonly open: boolean;
	/** The project it opened for, or null to start from the one you're looking at. */
	readonly project: ProjectId | null;
	/** Counts each opening, so the form starts fresh every time. */
	readonly opened: number;
};

const stores = new WeakMap<Services, StoreApi<NewAgentState>>();

/** The new-agent form's store for one window. */
export const newAgentOf = (services: Services): StoreApi<NewAgentState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<NewAgentState>(() => ({ open: false, project: null, opened: 0 }));
		stores.set(services, store);
	}
	return store;
};

/** The project of the agent you're looking at, if it has one. */
export const lookingAt = (services: Services): ProjectId | null => {
	const id = services.ui.getState().selectedAgent;
	const { tables } = services.view.getState().view;
	const agent = id ? tables.agent.get(id) : undefined;
	return (agent && tables.workspace.get(agent.workspace)?.project) ?? null;
};

/** Opens the form, for a project or for the one you're looking at. */
export const openNewAgent = (services: Services, project?: ProjectId | null): void =>
	newAgentOf(services).setState((s) => ({
		open: true,
		project: project ?? lookingAt(services),
		opened: s.opened + 1,
	}));

/** Closes the form. */
export const closeNewAgent = (services: Services): void =>
	newAgentOf(services).setState({ open: false });

/** Reads the form's state. */
export const useNewAgent = <T>(selector: (state: NewAgentState) => T): T =>
	useStore(newAgentOf(useServices()), selector);
