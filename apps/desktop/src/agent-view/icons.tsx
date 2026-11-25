// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent kinds' marks and the small spinner the timeline uses. A local
// agent gets a plain chip, never a vendor's mark, and one on a cloud
// service a cloud.

import type { ReactNode } from "react";

import type { Agent } from "@maxhq/shared/bindings";
import {
	agentKind,
	agentMark,
	type AgentKind,
	type KindMark,
} from "@maxhq/shared/status/agent-kind";
import { useServicesIfAny } from "@/app/services";
import { cn } from "@/lib/utils";

const MARK: Record<KindMark, ReactNode> = {
	claude: (
		<path d="M12 3v6M12 15v6M3 12h6M15 12h6M5.6 5.6l4.3 4.3M14.1 14.1l4.3 4.3M18.4 5.6l-4.3 4.3M9.9 14.1l-4.3 4.3" />
	),
	codex: (
		<>
			<path d="M12 3l7.8 4.5v9L12 21l-7.8-4.5v-9z" />
			<path d="M10 9.5L7.5 12l2.5 2.5M14 9.5l2.5 2.5-2.5 2.5" />
		</>
	),
	cursor: <path d="M5 3.5l13.5 7.2-6 1.6-2.6 5.7z" />,
	local: (
		<>
			<rect x="6" y="6" width="12" height="12" rx="2" />
			<path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4" />
		</>
	),
	acp: (
		<>
			<rect x="4" y="7" width="16" height="12" rx="2" />
			<path d="M12 3v4M9 13h.01M15 13h.01" />
		</>
	),
	cloud: <path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z" />,
};

/**
 * Claude's asterisk, Codex's hexagon, Cursor's pointer, Local's chip, a
 * cloud service's cloud or another agent's robot, drawn in the current
 * color. Pass the mark from `agentMark` or the kind from `agentKind`, so a
 * local agent never gets Claude's mark.
 */
export const VendorIcon = ({
	kind,
	size = 12,
	className,
}: {
	kind: KindMark;
	size?: number;
	className?: string;
}) => (
	<svg
		aria-hidden="true"
		width={size}
		height={size}
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		strokeLinecap="round"
		strokeLinejoin="round"
		strokeWidth={kind === "claude" ? 2.4 : 2}
		className={cn("shrink-0", className)}
		data-mark={kind}
	>
		{MARK[kind]}
	</svg>
);

type KindOf = Pick<Agent, "vendor" | "account" | "local">;

/**
 * What an agent shows as, its account looked up in the view. For a place
 * that has the agent but not the view, such as a menu row or a pill; outside
 * a services provider it goes by the agent's own record.
 */
export const useAgentKind = (agent: KindOf): AgentKind =>
	agentKind(agent, useServicesIfAny()?.view.getState().view.tables.account ?? []);

/** The mark an agent shows, as `useAgentKind` finds its kind: a cloud for a cloud service's. */
export const useAgentMark = (agent: KindOf): KindMark => {
	const tables = useServicesIfAny()?.view.getState().view.tables;
	return agentMark(agent, tables?.account ?? [], tables?.model_server);
};

/** An agent's mark, as `useAgentMark` finds it. */
export const AgentIcon = ({
	agent,
	size,
	className,
}: {
	agent: KindOf;
	size?: number;
	className?: string;
}) => <VendorIcon kind={useAgentMark(agent)} size={size} className={className} />;

/** A ring that turns while a step runs. */
export const Spinner = ({ size = 12, className }: { size?: number; className?: string }) => (
	<span
		aria-hidden="true"
		className={cn(
			"inline-block shrink-0 animate-busy rounded-full border-2 border-status-busy/25 border-t-status-busy",
			className
		)}
		style={{ width: size, height: size }}
	/>
);
