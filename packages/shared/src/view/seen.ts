// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Looking at an agent, on the Mac or the phone. A done agent you look at is
// marked seen, so its unread ring clears on every client, and one back
// from a snooze loses its Woke marker.

import { t } from "../i18n";

import type { Agent, AgentId, Timestamp } from "../bindings";
import type { DataSource } from "../data/source";

import { inboxOf } from "./inbox";
import { inboxFailureOf } from "./inbox-actions";
import type { ViewStore } from "./store";

/** What looking needs: the source to tell and the view to read. */
export type Looking = { readonly source: DataSource; readonly view: ViewStore };

const inFlight = <T>(map: WeakMap<DataSource, T>, source: DataSource, empty: () => T): T => {
	let value = map.get(source);
	if (value === undefined) {
		value = empty();
		map.set(source, value);
	}
	return value;
};

// Agents with a mark-seen call in flight, so a burst of deltas sends one.
const marking = new WeakMap<DataSource, Set<AgentId>>();

const markSeen = (source: DataSource, id: AgentId): void => {
	const pending = inFlight(marking, source, () => new Set<AgentId>());
	if (pending.has(id)) return;
	pending.add(id);
	source.markSeen(id).then(
		(result) => {
			pending.delete(id);
			if (result.status === "error")
				console.warn(t("Couldn't mark the agent seen"), result.error);
		},
		(error: unknown) => {
			pending.delete(id);
			console.warn(t("Couldn't mark the agent seen"), error);
		}
	);
};

// Leads with a dismiss call in flight, and the wake a call failed for, so
// looking doesn't try again on every delta. Opening the agent does.
const dismissing = new WeakMap<DataSource, Set<AgentId>>();
const failedWake = new WeakMap<DataSource, Map<AgentId, Timestamp>>();

const dismissWoke = (source: DataSource, lead: Agent, again: boolean): void => {
	const wokeAt = inboxOf(lead).woke_at;
	if (wokeAt === null) return;
	const pending = inFlight(dismissing, source, () => new Set<AgentId>());
	const failed = inFlight(failedWake, source, () => new Map<AgentId, Timestamp>());
	if (pending.has(lead.id) || (!again && failed.get(lead.id) === wokeAt)) return;
	pending.add(lead.id);
	const fail = (why: unknown) => {
		failed.set(lead.id, wokeAt);
		console.warn(t("Couldn't clear the Woke marker"), why);
	};
	source.setInbox([lead.id], { change: "dismiss_woke" }).then(
		([result]) => {
			pending.delete(lead.id);
			const failure = result ? inboxFailureOf(result) : null;
			if (failure !== null) fail(failure);
		},
		(error: unknown) => {
			pending.delete(lead.id);
			fail(error);
		}
	);
};

/**
 * Marks the agent seen when it's done and unread, and clears its lead's
 * Woke marker. A clear that failed isn't tried again for the same wake
 * unless `again` is set, as it is when you open the agent rather than
 * keep looking at it.
 */
export const lookAt = ({ source, view }: Looking, id: AgentId, again: boolean): void => {
	const state = view.getState().view;
	const agent = state.tables.agent.get(id);
	if (!agent) return;
	if (agent.activity.state === "done_unread") markSeen(source, id);
	// The marker sits on the lead's row, which a sub-agent shows beside.
	const lead = agent.parent === null ? agent : state.tables.agent.get(agent.parent);
	if (lead) dismissWoke(source, lead, again);
};
