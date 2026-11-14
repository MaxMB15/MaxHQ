// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The header's activity dot. While the agent is done but left something
// running, or waits on a background task, a click opens the Running list.

import type { Agent, Timestamp } from "@maxhq/shared/bindings";
import { t } from "@maxhq/shared/i18n";
import { agentLook } from "@maxhq/shared/status/activity";
import type { ViewState } from "@maxhq/shared/view/replica";

import { AgentActivityDot } from "@/components/status/agent-dots";

export const HeaderActivityDot = ({
	agent,
	view,
	now,
	onShowRunning,
}: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	/** Opens the Running list, when the header has one. */
	onShowRunning?: () => void;
}) => {
	const look = agentLook(agent, now);
	const dot = <AgentActivityDot agent={agent} view={view} now={now} />;
	if (!onShowRunning || (look !== "idle_running" && look !== "idle_waiting")) return dot;
	return (
		<button
			type="button"
			aria-label={t("Show what still runs")}
			onClick={onShowRunning}
			className="-m-1 inline-grid shrink-0 cursor-pointer place-items-center rounded-full p-1 outline-none hover:bg-secondary focus-visible:ring-2 focus-visible:ring-ring"
		>
			{dot}
		</button>
	);
};
