// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a list of a lead's sub-agents says: whose login the lead runs on,
// where and how many sub-agents it has, what the lead itself is doing, and
// how full its context window is. The desktop's slide-out and the phone's
// Sub-agents screen both show it.

import { formatNumber, t } from "../i18n";

import type { Activity, Agent } from "../bindings";
import type { ViewState } from "../view/replica";
import { deviceName } from "../view/select";

import { subAgentStatus } from "./activity";
import { accountName, VENDOR_NAME } from "./usage";

/** The list's second line, such as "Claude Max on devbox, 4 sub-agents". */
export const subAgentsSubtitle = (view: ViewState, lead: Agent, count: number): string => {
	const account = lead.account ? view.tables.account.get(lead.account) : undefined;
	const login = account ? accountName(account) : VENDOR_NAME[lead.vendor];
	const workspace = view.tables.workspace.get(lead.workspace);
	const where = workspace
		? t(" on {{value1}}", { value1: deviceName(view, workspace.device) })
		: "";
	return t("{{login}}{{where}}, {{countText}} sub-agents", {
		login,
		where,
		count,
		countText: formatNumber(count),
	});
};

/** What the lead's row says on its right: what the lead itself is doing. */
export const leadStatus = (activity: Activity): string => {
	switch (activity.state) {
		case "busy_in_subagents":
			return t("Waiting");
		case "done_unread":
		case "idle":
		case "idle_cache_warm":
			return t("Idle");
		default:
			return subAgentStatus(activity);
	}
};

/** How full an agent's context window is, from 0 to 100, or null when it doesn't say. */
export const contextShare = (context: Agent["context"]): number | null =>
	context && context.window_tokens > 0
		? Math.min(100, (context.used_tokens / context.window_tokens) * 100)
		: null;
