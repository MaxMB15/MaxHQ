// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Deleting agents from the sidebar's menus or an agent's header. It asks
// first and deletes them one after another. Then it unpicks them, opens
// the next agent in the project in place of the one you were looking at,
// and closes the panes of the agents that went.

import { t } from "@maxhq/shared/i18n";

import { requestConfirm } from "@/components/confirm";
import { paneId } from "@/layout/pane-id";
import { panesOf } from "@/layout/panes";
import type { Agent, AgentId } from "@maxhq/shared/bindings";
import { agentAfterDelete, deleteAgents, deleteQuestion } from "@maxhq/shared/ui/delete-agents";
import { all, type ViewState } from "@maxhq/shared/view/replica";

import { openAgent } from "./open-agent";
import type { Services } from "./services";

/**
 * Asks, then deletes the agents. Resolves with what went wrong, one
 * sentence for each reason, for the caller to show.
 */
export const confirmAndDelete = async (
	services: Services,
	agents: readonly Agent[]
): Promise<readonly string[]> => {
	if (agents.length === 0) return [];
	const yes = await requestConfirm(services, { ...deleteQuestion(agents), action: t("Delete") });
	if (!yes) return [];
	const before = services.view.getState().view;
	const { deleted, failures } = await deleteAgents(services.source, agents);
	if (deleted.size > 0) leave(services, before, deleted);
	return failures;
};

const leave = (services: Services, before: ViewState, deleted: ReadonlySet<AgentId>) => {
	const { ui } = services;
	const now = services.view.getState().view;
	ui.getState().unpickDeleted(deleted, (agent) => now.tables.agent.has(agent));
	// Their sub-agents went with them.
	const gone = all(before, "agent").filter(
		(agent) => deleted.has(agent.id) || (agent.parent !== null && deleted.has(agent.parent))
	);
	const selected = gone.find((agent) => agent.id === ui.getState().selectedAgent);
	if (selected) {
		const next = agentAfterDelete(before, selected.parent ?? selected.id, deleted);
		if (next !== null && now.tables.agent.has(next)) openAgent(services, next);
		else ui.setState({ selectedAgent: null, slideOut: null });
	}
	const api = panesOf(services).api();
	for (const agent of gone)
		api?.getPanel(paneId({ kind: "agent", agent: agent.id }))?.api.close();
};
