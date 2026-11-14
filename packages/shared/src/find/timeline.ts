// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Find in one agent's timeline, as the desktop and the phone both use it:
// asking maxd through the data source, and asking again as the timeline
// changes.

import type { AgentId, TimelineFind } from "../bindings";
import type { DataSource } from "../data/source";
import type { Timeline } from "../timeline/store";
import type { Find, FindAsk } from "./store";

const NO_MATCHES: TimelineFind = { total: 0, active: 0, found: null, near: [] };

// maxd turns down a find in an agent with nothing to read yet, which has no matches.
const unreadable = (error: unknown) =>
	error instanceof Error && error.message.includes("can't read that agent's timeline");

/** Asks maxd to find a query in the agent's timeline. */
export const timelineAsk =
	(source: Pick<DataSource, "findInTimeline">, agent: AgentId): FindAsk =>
	(query, at) =>
		source
			.findInTimeline(agent, query, at)
			.catch((error: unknown) => (unreadable(error) ? NO_MATCHES : Promise.reject(error)));

/** Tells `find` each time the timeline changes, so it asks again. Returns the undo. */
export const followTimeline = (find: Find, timeline: Timeline): (() => void) =>
	timeline.store.subscribe((state, before) => {
		if (state.seq !== before.seq) find.changed();
	});
