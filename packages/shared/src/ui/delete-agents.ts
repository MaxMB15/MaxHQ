// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Deleting agents, from a menu or an agent's header: which of them a
// delete takes, what the confirmation asks, the deletes one after another
// for the agents you selected, and the agent to open
// in place of the one you were looking at.

import { t } from "../i18n";

import type { Agent, AgentId } from "../bindings";
import type { DataSource } from "../data/source";
import { plural, sentence } from "../status/format";
import { type Failed, inboxFailureOf, inboxTargets, triedLines } from "../view/inbox-actions";
import { all, type ViewState } from "../view/replica";
import { byRank } from "../view/select";

/**
 * The agents of these a delete takes: the leads MaxHQ holds. A sub-agent
 * goes with its lead, and one you started in a terminal stays.
 */
export const deletableAgents = (view: ViewState, ids: Iterable<AgentId>): Agent[] =>
	inboxTargets(view, ids).filter((agent) => agent.control === "held");

/** What the confirmation asks before deleting these agents, and what it means. */
export type DeleteQuestion = { readonly title: string; readonly detail: string };

/** The confirmation for deleting these agents, naming the agent when there's one. */
export const deleteQuestion = (agents: readonly Agent[]): DeleteQuestion =>
	agents.length === 1
		? {
				title: t('Delete agent "{{agents_0___title}}"?', {
					agents_0___title: agents[0]!.title,
				}),
				detail: t(
					"MaxHQ stops it and won't show it again. Its folder and its CLI's transcript stay."
				),
			}
		: {
				title: t("Delete {{plural_agents_length___agent}}?", {
					plural_agents_length___agent: plural(agents.length, "agent"),
				}),
				detail: t(
					"MaxHQ stops them and won't show them again. Their folders and their CLIs' transcripts stay."
				),
			};

/** How a delete went. */
export type DeleteOutcome = {
	/** The agents it deleted. */
	readonly deleted: ReadonlySet<AgentId>;
	/** What went wrong for the rest, one sentence for each reason. */
	readonly failures: readonly string[];
};

/**
 * Deletes the agents one after another, so each device gets one request
 * at a time, and keeps going past a failure.
 */
export const deleteAgents = async (
	source: Pick<DataSource, "deleteAgent">,
	agents: readonly Agent[]
): Promise<DeleteOutcome> => {
	const deleted = new Set<AgentId>();
	const failed: Failed[] = [];
	for (const agent of agents) {
		let reason: string | null;
		try {
			reason = inboxFailureOf(await source.deleteAgent(agent.id));
		} catch (error) {
			reason = sentence(error instanceof Error ? error.message : String(error));
		}
		if (reason === null) deleted.add(agent.id);
		else failed.push({ agent, reason });
	}
	return { deleted, failures: triedLines("delete", failed) };
};

/**
 * The agent to open in place of a deleted one: the first lead left in its
 * project, in the order the sidebar sorts them. Null when none is left. `view` is from before the delete.
 */
export const agentAfterDelete = (
	view: ViewState,
	agent: AgentId,
	deleted: ReadonlySet<AgentId>
): AgentId | null => {
	const projectOf = (one: Agent) => view.tables.workspace.get(one.workspace)?.project;
	const gone = view.tables.agent.get(agent);
	if (!gone) return null;
	const project = projectOf(gone);
	const left = all(view, "agent").filter(
		(other) =>
			other.parent === null &&
			other.id !== agent &&
			!deleted.has(other.id) &&
			projectOf(other) === project
	);
	return left.sort(byRank)[0]?.id ?? null;
};
