// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A workflow run's right-click menu, on its row in the Workflows entry and
// on the rows of its Workflow tab: Open, Open in split, Stop run while it's
// live, which asks first, and Copy run ID. Opening a run goes to its lead's
// Workflow tab when an agent leads it, and to run mode when none does.

import { t, useI18n } from "@maxhq/shared/i18n";

import { Copy, ExternalLink, PanelRight, Square } from "lucide-react";

import { openAgent } from "@/app/open-agent";
import { type Services, useServices } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { panesOf } from "@/layout/panes";
import { noticesOf } from "@/sidebar/notices";
import { MenuItem } from "@/sidebar/project-menu";
import type { AgentId, WorkflowRun } from "@maxhq/shared/bindings";
import { describe } from "@maxhq/shared/data/request-source";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";
import { stopRunQuestion, stopTargets } from "@maxhq/shared/workflow/stop";

/** Opens a run: its lead's Workflow tab when an agent leads it, run mode when none does. */
export const openRunOf = (services: Services, run: WorkflowRun) => {
	const lead = run.orchestrators[0];
	if (lead !== undefined && services.view.getState().view.tables.agent.has(lead)) {
		openAgent(services, lead);
		services.ui.getState().openRun(run.key, lead);
	} else {
		services.ui.getState().openRun(run.key);
	}
};

/** Stops the turns of the agents working on a run, once you say yes to what that loses. */
const askToStop = async (services: Services, run: WorkflowRun, agents: readonly AgentId[]) => {
	const yes = await requestConfirm(services, {
		...stopRunQuestion(run, agents.length),
		action: t("Stop run"),
	});
	if (!yes) return;
	const results = await Promise.all(agents.map((agent) => services.source.interrupt(agent)));
	const failed = results.find((result) => result.status === "error");
	if (failed?.status === "error") showFailure(noticesOf(services), describe(failed.error));
};

/**
 * A run's menu items. `agent` adds Open and Open in split for a node's
 * agent, on a row of the Workflow tab, in place of opening the run.
 */
export const RunMenuItems = ({
	run,
	agent,
	onOpenAgent,
}: {
	run: WorkflowRun;
	agent?: AgentId | null;
	onOpenAgent?: (agent: AgentId, split: boolean) => void;
}) => {
	useI18n();

	const services = useServices();
	const nodeAgent = agent && onOpenAgent ? agent : null;
	const stoppable = stopTargets(services.view.getState().view, run);
	return (
		<>
			{nodeAgent !== null ? (
				<>
					<MenuItem
						icon={ExternalLink}
						label={t("Open agent")}
						onClick={() => onOpenAgent?.(nodeAgent, false)}
					/>
					<MenuItem
						icon={PanelRight}
						label={t("Open agent in split")}
						onClick={() => onOpenAgent?.(nodeAgent, true)}
					/>
				</>
			) : (
				<>
					<MenuItem
						icon={ExternalLink}
						label={t("Open")}
						onClick={() => openRunOf(services, run)}
					/>
					<MenuItem
						icon={PanelRight}
						label={t("Open in split")}
						onClick={() => panesOf(services).openRun(run.key, { split: true })}
					/>
				</>
			)}
			{stoppable.length > 0 ? (
				<MenuItem
					icon={Square}
					label={t("Stop run…")}
					onClick={() => void askToStop(services, run, stoppable)}
				/>
			) : null}
			<MenuItem
				icon={Copy}
				label={t("Copy run ID")}
				onClick={() => void navigator.clipboard.writeText(run.key.run).catch(console.error)}
			/>
		</>
	);
};
