// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Stop run, from a workflow run's right-click menu: which agents it stops
// and what it asks first.

import { t } from "../i18n";

import type { AgentId, WorkflowRun } from "../bindings";
import type { ViewState } from "../view/replica";

/**
 * The agents Stop run stops the turn of: the run's leads, then the agents
 * its nodes are bound to, those the view still has. None for a run that
 * finished.
 */
export const stopTargets = (view: ViewState, run: WorkflowRun): AgentId[] =>
	run.done
		? []
		: [...new Set([...run.orchestrators, ...(run.agents ?? [])])].filter((agent) =>
				view.tables.agent.has(agent)
			);

/** What Stop run asks before it stops the turns of `agents` agents. */
export const stopRunQuestion = (
	run: WorkflowRun,
	agents: number
): { readonly title: string; readonly detail: string } => ({
	title: t('Stop "{{label}}"?', { label: run.label }),
	detail:
		agents === 1
			? t(
					"This stops the current turn of the agent working on it, and what it was in the middle of is lost. The agent stays open, so you can tell it to carry on."
				)
			: t(
					"This stops the current turn of the {{count}} agents working on it, and what they were in the middle of is lost. The agents stay open, so you can tell them to carry on.",
					{ count: agents }
				),
});
