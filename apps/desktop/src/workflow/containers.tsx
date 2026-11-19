// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where the workflow views meet the stores: the slide-out with its tabs,
// the Workflow tab's run, the chips on a lead's row and header, and a
// project's Workflows entry. Each reads the view, the UI store's slide-out
// and the run's store, and hands the views plain values and callbacks.

import { useI18n } from "@maxhq/shared/i18n";

import { ChevronsDownUp, ChevronsUpDown } from "lucide-react";
import { useCallback, useMemo } from "react";

import { openAgent } from "@/app/open-agent";
import { panesOf } from "@/layout/panes";
import { useServices, useUi, useView } from "@/app/services";
import { useRunHeader, useRunLoad, useRunLoops, useRunNodes, useRunShape } from "@/app/use-run";
import { MenuItem } from "@/sidebar/project-menu";
import { SlideOut, slideOutWidth, SubAgentList } from "@/sidebar/sub-agents";
import type {
	Agent,
	AgentId,
	NodeDetail,
	Project,
	RunKey,
	Timestamp,
	WorkflowRun,
	WorkflowRunId,
} from "@maxhq/shared/bindings";
import { subAgentsSubtitle } from "@maxhq/shared/status/sub-agents";
import { all } from "@maxhq/shared/view/replica";
import { childrenOf } from "@maxhq/shared/view/select";
import { finishedLead, nameSubAgent } from "@maxhq/shared/agent-view/finished";
import { foldWorkflows, workflowsFolded } from "@maxhq/shared/workflow/fold";
import { projectRuns, runAt, runChip, runsOf, sameRun } from "@maxhq/shared/workflow/select";

import { HeaderRunChip, RowRunChip, WorkflowsEntry } from "./chips";
import { openRunOf, RunMenuItems } from "./run-menu";
import { WorkflowTab } from "./workflow-tab";

/** The latest run a lead leads, which keeps its object until it changes. */
const useLatestRun = (lead: AgentId): WorkflowRun | undefined =>
	useView((s) => runsOf(s.view, lead)[0]);

/**
 * Opens a node's agent: beside the focused pane when `split`, and in place
 * for the lead's own sub-agents, as on the Sub-agents tab, even once they
 * finished. A finished one's pane takes its node's `label`.
 */
export const useOpenNodeAgent = (lead: AgentId | undefined) => {
	const services = useServices();
	return useCallback(
		(agent: AgentId, split: boolean, label?: string) => {
			const view = services.view.getState().view;
			if (label !== undefined) nameSubAgent(view, agent, label);
			const parent = view.tables.agent.get(agent)?.parent ?? finishedLead(view, agent)?.id;
			if (!split && lead !== undefined && parent === lead) {
				services.ui.getState().selectSubAgent(agent);
			} else {
				openAgent(services, agent, { split });
			}
		},
		[services, lead]
	);
};

/** Reads a node's detail lists, or null when the source has none. */
export const useNodeDetail = (run: RunKey) => {
	const services = useServices();
	return useCallback(
		async (node: string): Promise<NodeDetail | null> => {
			const result = await services.source.nodeDetail(run, node);
			return result.status === "ok" ? result.data : null;
		},
		[services, run]
	);
};

/** The Workflow tab for a run, with a lead or in run mode. */
const WorkflowTabFor = ({
	lead,
	run,
	runs,
}: {
	lead: Agent | undefined;
	run: WorkflowRun;
	runs: readonly WorkflowRun[];
}) => {
	useI18n();

	const services = useServices();
	const selected = useUi((s) => s.selectedAgent);
	const shape = useRunShape(run.key);
	const nodes = useRunNodes(run.key);
	const loops = useRunLoops(run.key);
	const header = useRunHeader(run.key);
	const load = useRunLoad(run.key);
	const leadId = lead?.id;
	const onOpenAgent = useOpenNodeAgent(leadId);
	const onDetail = useNodeDetail(run.key);
	const rowMenu = useCallback(
		(agent: AgentId | null) => (
			<RunMenuItems run={run} agent={agent} onOpenAgent={onOpenAgent} />
		),
		[run, onOpenAgent]
	);
	const onPickRun = useCallback(
		(id: WorkflowRunId) => {
			const picked = runs.find((r) => r.id === id);
			if (picked) services.ui.getState().openRun(picked.key, leadId ?? null);
		},
		[services, runs, leadId]
	);
	return (
		<WorkflowTab
			runs={runs}
			run={run}
			shape={shape}
			nodes={nodes}
			loops={loops}
			header={header}
			load={load}
			selected={selected}
			named={lead !== undefined}
			onPickRun={onPickRun}
			onOpenAgent={onOpenAgent}
			onOpenCanvas={() => panesOf(services).openRun(run.key)}
			onDetail={onDetail}
			rowMenu={rowMenu}
		/>
	);
};

/** The slide-out for the open lead, or for a run with no lead. Nothing when neither is open. */
export const SlideOutFor = ({ now }: { now: Timestamp }) => {
	const { t } = useI18n();

	const slide = useUi((s) => s.slideOut);
	const close = useUi((s) => s.closeSubAgents);
	const setTab = useUi((s) => s.setSlideTab);
	const view = useView((s) => s.view);
	const lead = slide?.lead ? view.tables.agent.get(slide.lead) : undefined;
	const runs = useMemo(() => (lead ? runsOf(view, lead.id) : []), [view, lead]);

	if (lead) {
		const run = runs.find((r) => sameRun(r.key, slide?.run ?? null)) ?? runs[0];
		const tab = run ? (slide?.tab ?? "agents") : "agents";
		return (
			<SlideOut
				label={t("Sub-agents of {{title}}", { title: lead.title })}
				title={lead.title}
				subtitle={subAgentsSubtitle(view, lead, childrenOf(view, lead.id).length)}
				width={slideOutWidth(run !== undefined)}
				closeLabel="Close sub-agents"
				onClose={close}
				tab={tab}
				onTab={run ? setTab : undefined}
			>
				{run && tab === "workflow" ? (
					<WorkflowTabFor lead={lead} run={run} runs={runs} />
				) : (
					<SubAgentList lead={lead} now={now} />
				)}
			</SlideOut>
		);
	}

	const run = slide?.run ? runAt(view, slide.run) : undefined;
	if (!run) return null;
	const project = view.tables.project.get(run.key.project);
	return (
		<SlideOut
			label={t("Workflow run {{label}}", { label: run.label })}
			title={run.label}
			subtitle={project?.name ?? run.key.workflow}
			width={slideOutWidth(true)}
			closeLabel="Close workflow"
			onClose={close}
		>
			<WorkflowTabFor lead={undefined} run={run} runs={[run]} />
		</SlideOut>
	);
};

/** The latest run's chip on a lead's sidebar row, which opens the agent on its Workflow tab. */
export const RowRunChipFor = ({ agent }: { agent: AgentId }) => {
	useI18n();

	const services = useServices();
	const run = useLatestRun(agent);
	const chip = run ? runChip(run) : null;
	if (!chip) return null;
	return (
		<RowRunChip
			chip={chip}
			onOpen={() => {
				openAgent(services, agent);
				services.ui.getState().openRun(chip.run, agent);
			}}
		/>
	);
};

/** The latest run's chip in an agent's header, which opens its Workflow tab. */
export const HeaderRunChipFor = ({ agent }: { agent: AgentId }) => {
	useI18n();

	const openRun = useUi((s) => s.openRun);
	const run = useLatestRun(agent);
	const chip = run ? runChip(run) : null;
	if (!chip) return null;
	return <HeaderRunChip chip={chip} onOpen={() => openRun(chip.run, agent)} />;
};

/** A project's Workflows entry, for every run in it, bound to a lead or not. */
export const ProjectWorkflows = ({ project, now }: { project: Project; now: Timestamp }) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const runs = useMemo(() => projectRuns(view, project.id), [view, project.id]);
	const folded = useUi((s) => workflowsFolded(s.collapsed, project.id, runs.length));
	const refold = useUi((s) => s.refold);
	// The run the slide-out shows, with its lead or in run mode.
	const shown = useUi((s) => s.slideOut?.run ?? null);
	const showing = runs.find((run) => sameRun(run.key, shown))?.id ?? null;
	const foldAll = (fold: boolean) => {
		const projects = [...new Set(all(view, "workflow_run").map((run) => run.key.project))];
		refold((collapsed) => foldWorkflows(collapsed, projects, fold));
	};
	return (
		<WorkflowsEntry
			project={project.name}
			runs={runs}
			folded={folded}
			showing={showing}
			now={now}
			onToggle={() => refold((collapsed) => foldWorkflows(collapsed, [project.id], !folded))}
			onOpen={(id) => {
				const run = runs.find((r) => r.id === id);
				if (run) openRunOf(services, run);
			}}
			entryMenu={
				<>
					<MenuItem
						icon={ChevronsDownUp}
						label={t("Collapse all")}
						onClick={() => foldAll(true)}
					/>
					<MenuItem
						icon={ChevronsUpDown}
						label={t("Expand all")}
						onClick={() => foldAll(false)}
					/>
				</>
			}
			runMenu={(run) => <RunMenuItems run={run} />}
		/>
	);
};
