// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A sub-agent that finished. maxd stops listing it, but Claude Code keeps
// its transcript beside its lead's, so its pane reads it from there, read
// only, through the lead, for as long as the lead is listed. The window
// learns a sub-agent's lead when its view shows, or from the sub-agents run
// it's bound to. The desktop and the phone share this.

import { t } from "../i18n";

import { useEffect } from "react";

import { type Agent, type AgentId, FINISHED_SUB_AGENTS_SINCE } from "../bindings";
import { VENDOR_NAME } from "../status/usage";
import type { ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";
import { keep, keptValue } from "./kept";
import { rootLead } from "./outside";

/** The workflow whose runs list a lead's sub-agents. */
const SUB_AGENTS = "maxhq.agents";

/** What the window saw of a sub-agent while it was listed. */
type Seen = { readonly lead: AgentId; readonly title: string | null };

const SEEN = "sub-agent:";

/**
 * Remembers that the sub-agent `agent` runs under `lead`, at the top of its
 * tree, so its pane can still open once it finishes. A null title keeps the
 * one seen before.
 */
export const keepSubAgent = (agent: AgentId, lead: AgentId, title: string | null) => {
	if (lead === agent) return;
	const seen = keptValue<Seen | null>(SEEN + agent, null);
	keep(SEEN + agent, { lead, title: title ?? (seen?.lead === lead ? seen.title : null) });
};

/** Remembers a sub-agent's lead and title while its view shows. */
export const useKeepSubAgent = (agent: Agent, lead: AgentId | null) => {
	useEffect(() => {
		if (lead !== null) keepSubAgent(agent.id, lead, agent.title);
	}, [agent.id, agent.title, lead]);
};

/**
 * Remembers the sub-agent a card in `holder`'s feed opens, under the lead at
 * the top of `holder`'s tree, so it opens even once it finished.
 */
export const keepCardSubAgent = (
	view: ViewState,
	holder: AgentId,
	agent: AgentId,
	title: string
) => {
	const found = view.tables.agent.get(holder);
	const lead = found && rootLead(view.tables.agent, found);
	if (lead) keepSubAgent(agent, lead.id, title);
};

/** The lead at the top of a sub-agent's tree, or null for a lead. */
export const subAgentLead = (agents: ReadonlyMap<string, Agent>, agent: Agent): AgentId | null =>
	agent.parent === null ? null : (rootLead(agents, agent)?.id ?? null);

/** The lead of the sub-agents run that `agent` is bound to. */
const runLead = (view: ViewState, agent: AgentId): AgentId | null => {
	for (const run of view.tables.workflow_run.values())
		if (run.key.workflow === SUB_AGENTS && run.agents?.includes(agent))
			return run.orchestrators[0] ?? null;
	return null;
};

/** What the pane of an agent the view doesn't list can show. */
const finished = (view: ViewState, id: AgentId): Agent | string | null => {
	const agents = view.tables.agent;
	if (agents.has(id)) return null;
	const known = keptValue<Seen | null>(SEEN + id, null)?.lead ?? runLead(view, id);
	if (known === null) return null;
	const found = agents.get(known);
	const lead = found && rootLead(agents, found);
	if (!lead) return t("This sub-agent finished, and its lead is no longer here.");
	if (lead.vendor !== "claude")
		return t(
			"This sub-agent finished. MaxHQ shows the history of finished sub-agents only for {{vendor}}.",
			{ vendor: VENDOR_NAME.claude }
		);
	const workspace = view.tables.workspace.get(lead.workspace);
	// A guest reads only a session's own history.
	if (workspace?.location.type === "container")
		return t(
			"This sub-agent finished. MaxHQ can't show the history of finished sub-agents in a container yet."
		);
	const protocol = workspace ? (linkForDevice(view, workspace.device)?.protocol ?? null) : null;
	if (workspace && protocol !== null && protocol < FINISHED_SUB_AGENTS_SINCE)
		return t("This sub-agent finished. Showing its history needs a newer maxd on {{device}}.", {
			device: deviceName(view, workspace.device),
		});
	return lead;
};

/**
 * The lead whose session holds the history of `id`, a sub-agent that
 * finished, while its pane can show it. Null for a listed agent, or one
 * the window knows nothing of.
 */
export const finishedLead = (view: ViewState, id: AgentId): Agent | null => {
	const found = finished(view, id);
	return typeof found === "object" ? found : null;
};

/** Why the pane of `id`, a sub-agent that finished, can't show its history. */
export const finishedNote = (view: ViewState, id: AgentId): string | null => {
	const found = finished(view, id);
	return typeof found === "string" ? found : null;
};

/** The title a finished sub-agent had while the window saw it, if it did. */
export const finishedTitle = (id: AgentId): string | null =>
	keptValue<Seen | null>(SEEN + id, null)?.title ?? null;

/**
 * Names a sub-agent opened from a run, when the window knows its lead, for
 * a pane opened once it finished.
 */
export const nameSubAgent = (view: ViewState, id: AgentId, title: string) => {
	if (view.tables.agent.has(id)) return;
	const lead = keptValue<Seen | null>(SEEN + id, null)?.lead ?? runLead(view, id);
	if (lead !== null) keep(SEEN + id, { lead, title });
};

/** What a finished sub-agent's pane says in place of the composer. */
export const finishedBanner = (): { readonly title: string; readonly body: string } => ({
	title: t("This sub-agent finished."),
	body: t("This is its history, read only. To follow up on its work, write to its lead."),
});
