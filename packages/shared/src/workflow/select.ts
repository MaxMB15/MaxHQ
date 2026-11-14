// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Read-only questions over the view's workflow runs: which runs a lead
// leads, which runs no agent leads yet, and what the chip on a lead says.

import { t } from "../i18n";

import type { AgentId, NodeStatus, ProjectId, RunKey, WorkflowRun } from "../bindings";
import { all, type ViewState } from "../view/replica";

/** One string per run, for maps and channel names. */
export const runName = (key: RunKey): string =>
	`${key.device}/${key.project}/${key.workflow}/${key.run}`;

/** Whether two keys name the same run. */
export const sameRun = (a: RunKey | null, b: RunKey | null): boolean =>
	a !== null && b !== null && runName(a) === runName(b);

const latestFirst = (a: WorkflowRun, b: WorkflowRun): number =>
	(b.last_activity ?? 0) - (a.last_activity ?? 0);

/** The runs an agent leads, latest first, which is the run picker's order. */
export const runsOf = (view: ViewState, lead: AgentId): WorkflowRun[] =>
	all(view, "workflow_run")
		.filter((run) => run.orchestrators.includes(lead))
		.sort(latestFirst);

/** A project's runs that no agent leads yet, latest first. */
export const unboundRuns = (view: ViewState, project: ProjectId): WorkflowRun[] =>
	all(view, "workflow_run")
		.filter((run) => run.key.project === project && run.orchestrators.length === 0)
		.sort(latestFirst);

/**
 * Every run in a project, bound to a lead or not: live ones first, then
 * finished ones, each latest first. This is the Workflows entry's order.
 */
export const projectRuns = (view: ViewState, project: ProjectId): WorkflowRun[] =>
	all(view, "workflow_run")
		.filter((run) => run.key.project === project)
		.sort((a, b) => Number(a.done) - Number(b.done) || latestFirst(a, b));

/**
 * The agents bound to the nodes of live runs, other than their leads. The
 * agent list leaves them out, since the Workflows entry shows their run.
 */
export const liveRunAgents = (view: ViewState): ReadonlySet<AgentId> => {
	const agents = new Set<AgentId>();
	for (const run of all(view, "workflow_run")) {
		if (!run.done) for (const agent of run.agents ?? []) agents.add(agent);
	}
	return agents;
};

/** A run's state as a node status, for its glyph: waiting on you, busy, failed or done. */
export const runStatus = (run: WorkflowRun): NodeStatus => {
	if (!run.done) return run.needs_you ? "waiting_human" : "busy";
	return run.failed ? "failed" : "done";
};

/** The run a key names, if the view has it. */
export const runAt = (view: ViewState, key: RunKey): WorkflowRun | undefined => {
	const name = runName(key);
	return all(view, "workflow_run").find((run) => runName(run.key) === name);
};

/** The agent that leads a run, once one does. */
export const leadOf = (view: ViewState, key: RunKey): AgentId | null =>
	runAt(view, key)?.orchestrators[0] ?? null;

/** What the chip on a lead's row and header says, for its latest run. */
export type RunChip = {
	readonly run: RunKey;
	/** The run's name, for its tooltip. */
	readonly label: string;
	/** The workflow and its progress, such as "pipeline 11/18". */
	readonly text: string;
	/** What it says to a screen reader, such as "Search filters, 11 of 18 done". */
	readonly spoken: string;
	readonly needsYou: boolean;
	readonly done: boolean;
};

/** A run's chip. */
export const runChip = (run: WorkflowRun): RunChip => {
	const { done, total } = run.progress;
	return {
		run: run.key,
		label: run.label,
		text: `${run.key.workflow} ${done}/${total}`,
		spoken: t("{{value1}}, {{value2}} of {{value3}} done{{value4}}", {
			value1: run.label,
			value2: done,
			value3: total,
			value4: run.needs_you ? t(", needs you") : "",
		}),
		needsYou: run.needs_you,
		done: run.done,
	};
};

/** The chip for a lead's latest run, or null when it leads none. */
export const chipOf = (view: ViewState, lead: AgentId): RunChip | null => {
	const [latest] = runsOf(view, lead);
	return latest ? runChip(latest) : null;
};
