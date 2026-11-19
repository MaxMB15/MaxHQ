// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone's Sub-agents screen lists, as the desktop's slide-out: the
// lead first, then each sub-agent, oldest first, with its dot and what it's
// doing. Each row holds plain values, so a row only draws again when what it
// shows changed. The agent's More sheet asks whether it has sub-agents, and
// how many.

import type { AgentId, Agent, Timestamp } from "@maxhq/shared/bindings";
import { type ActivityLook, agentLook, subAgentStatus } from "@maxhq/shared/status/activity";
import { leadStatus } from "@maxhq/shared/status/sub-agents";
import type { ViewState } from "@maxhq/shared/view/replica";
import { childrenOf } from "@maxhq/shared/view/select";

/** How many sub-agents `agent` has in the view. */
export const subAgentCount = (view: ViewState, agent: AgentId): number => {
	let count = 0;
	for (const other of view.tables.agent.values()) if (other.parent === agent) count += 1;
	return count;
};

/** Whether `agent` has a sub-agent in the view. */
export const hasSubAgents = (view: ViewState, agent: AgentId): boolean => {
	for (const other of view.tables.agent.values()) if (other.parent === agent) return true;
	return false;
};

/** One row of the list. */
export type SubAgentRow = {
	readonly id: AgentId;
	/** The sub-agent's title. The lead's row says Lead instead. */
	readonly title: string;
	readonly look: ActivityLook;
	/** What it's doing, on the row's right. */
	readonly status: string;
	readonly lead: boolean;
	/** Whether it's still starting, which dims its title. */
	readonly quiet: boolean;
};

/** The lead's row, then its sub-agents' rows, oldest first. */
export const subAgentRows = (view: ViewState, lead: Agent, now: Timestamp): SubAgentRow[] => [
	{
		id: lead.id,
		title: lead.title,
		look: agentLook(lead, now),
		status: leadStatus(lead.activity),
		lead: true,
		quiet: false,
	},
	...childrenOf(view, lead.id).map((child): SubAgentRow => ({
		id: child.id,
		title: child.title,
		look: agentLook(child, now),
		status: subAgentStatus(child.activity),
		lead: false,
		quiet: child.activity.state === "starting",
	})),
];
