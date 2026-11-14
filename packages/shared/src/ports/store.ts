// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the window remembers about ports beyond the view: the offers the
// user dismissed, and the forwards that stopped because their port closed,
// until they dismiss the notice. It lasts as long as the window does.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AgentId, PortId } from "../bindings";
import type { ViewStore } from "../view/store";

import { type ClosedForward, closedForwards } from "./select";

type PortsState = {
	/** Ports whose offer the user dismissed. They stay in the workspace popover. */
	readonly dismissed: ReadonlySet<PortId>;
	/** Forwards that stopped when their port closed, newest last. */
	readonly closed: readonly ClosedForward[];
};

const EMPTY: PortsState = { dismissed: new Set(), closed: [] };

const store = createStore<PortsState>(() => EMPTY);

const sameNotice = (a: ClosedForward, b: { agent: AgentId; port: number }) =>
	a.agent === b.agent && a.port === b.port;

/** Reads what the window remembers about ports. */
export const usePorts = <T>(selector: (state: PortsState) => T): T => useStore(store, selector);

/** Hides a port's offer. A port that opens again gets a new ID and a new offer. */
export const dismissPort = (port: PortId) =>
	store.setState((state) => ({ dismissed: new Set(state.dismissed).add(port) }));

/** Clears the notice that an agent's forward on a port stopped. */
export const dismissClosed = (agent: AgentId, port: number) =>
	store.setState((state) => ({
		closed: state.closed.filter((notice) => !sameNotice(notice, { agent, port })),
	}));

/** Remembers forwards that stopped, one notice per agent and port number. */
const noteClosed = (notices: readonly ClosedForward[]) => {
	if (notices.length === 0) return;
	store.setState((state) => ({
		closed: [
			...state.closed.filter((old) => !notices.some((notice) => sameNotice(old, notice))),
			...notices,
		],
	}));
};

/**
 * Watches the view for forwards that stop because their port closed, until
 * the returned function is called. Several watchers on one view note each
 * stop once.
 */
export const watchClosed = (view: ViewStore): (() => void) =>
	view.subscribe((state, previous) => {
		if (state.view !== previous.view) noteClosed(closedForwards(previous.view, state.view));
	});

/** Forgets everything, for tests. */
export const clearPorts = () => store.setState(EMPTY, true);
