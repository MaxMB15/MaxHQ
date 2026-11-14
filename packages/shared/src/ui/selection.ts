// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: how a click with Command
// or Shift changes the agents picked in the sidebar,
// as plain functions the window's store calls.

import type { AgentId } from "../bindings";

/** The agents picked in the sidebar, and the one a Shift-click counts from. */
export type Picked = {
	readonly picked: ReadonlySet<AgentId>;
	readonly anchor: AgentId | null;
};

/** Adds the agent, or takes it out when it's in. An agent added becomes the anchor. */
export const togglePick = (state: Picked, agent: AgentId): Picked => {
	const next = new Set(state.picked);
	if (next.has(agent)) next.delete(agent);
	else next.add(agent);
	return { picked: next, anchor: next.has(agent) ? agent : state.anchor };
};

/**
 * Adds every agent from the anchor to this one, in the order the sidebar
 * shows them, and keeps the anchor. With no anchor, or one the sidebar no
 * longer shows, it adds this one and makes it the anchor.
 */
export const pickRange = (state: Picked, agent: AgentId, ordered: readonly AgentId[]): Picked => {
	const next = new Set(state.picked);
	const from = state.anchor === null ? -1 : ordered.indexOf(state.anchor);
	const to = ordered.indexOf(agent);
	if (from === -1 || to === -1) {
		next.add(agent);
		return { picked: next, anchor: agent };
	}
	for (let i = Math.min(from, to); i <= Math.max(from, to); i += 1) next.add(ordered[i]!);
	return { picked: next, anchor: state.anchor };
};

/**
 * Takes out the agents a delete took and any the sidebar no longer has.
 * The anchor goes with its agent.
 */
export const unpickDeleted = (
	state: Picked,
	deleted: ReadonlySet<AgentId>,
	has: (agent: AgentId) => boolean
): Picked => {
	const gone = (agent: AgentId) => deleted.has(agent) || !has(agent);
	if (![...state.picked].some(gone)) return state;
	return {
		picked: new Set([...state.picked].filter((agent) => !gone(agent))),
		anchor: state.anchor !== null && gone(state.anchor) ? null : state.anchor,
	};
};
