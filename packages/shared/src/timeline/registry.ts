// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Open timelines, shared by every view of the same agent. One stays open
// for a minute after its last view goes, so a pane hidden and shown again
// finds its items and older pages where it left them.

import type { AgentId } from "../bindings";

import type { DataSource } from "../data/source";
import { LINGER_MS, sharedOpens } from "../data/shared-opens";
import { openTimeline, type Timeline } from "./store";

/** Every open timeline, by agent. */
export const timelineOpens = sharedOpens<AgentId, Timeline>(
	(source, agent) => openTimeline(source, agent),
	(agent) => agent
);

/**
 * The agent's open timeline, opened if it isn't. Safe to call while
 * rendering: it returns the same timeline each time, and one nobody leases
 * closes on its own.
 */
export const sharedTimeline = (
	source: DataSource,
	agent: AgentId,
	linger: number = LINGER_MS
): Timeline => timelineOpens.shared(source, agent, linger);

/** A timeline for one view to use, and the call that says the view is done with it. */
export type Lease = { readonly timeline: Timeline; readonly release: () => void };

/** Opens an agent's timeline, or shares the one already open. */
export const leaseTimeline = (
	source: DataSource,
	agent: AgentId,
	linger: number = LINGER_MS
): Lease => {
	const { value, release } = timelineOpens.hold(source, agent, linger);
	return { timeline: value, release };
};
