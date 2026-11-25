// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pane of a sub-agent that finished: its history, read once from its
// lead's session and never again, with find, and a note in place of the
// composer that points to its lead.

import { useCallback, useMemo, useState } from "react";

import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { ErrorBoundary } from "@/components/error-boundary";
import { isRemoteDevice } from "@/ports";
import type {
	Agent,
	AgentId,
	AttentionAnswer,
	AttentionId,
	InlineRef,
} from "@maxhq/shared/bindings";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { FindProvider } from "@maxhq/shared/agent-view/find-context";
import { finishedTitle } from "@maxhq/shared/agent-view/finished";
import { useI18n } from "@maxhq/shared/i18n";
import { agentKind, agentKindName, agentMark, kindName } from "@maxhq/shared/status/agent-kind";
import { accountName } from "@maxhq/shared/status/usage";

import { FindBar } from "./find/bar";
import { useFind } from "./find/use-find";
import { VendorIcon } from "./icons";
import type { JumpTarget } from "./jump";
import { SubAgentFinished } from "./outside-banner";
import { TimelineList } from "./timeline-list";
import { useTimeline } from "./use-timeline";

/** A finished sub-agent's history, which `lead`'s session holds. Fills its parent's height. */
export const FinishedView = ({ agent, lead }: { agent: AgentId; lead: Agent }) => {
	const { t } = useI18n();
	const services = useServices();
	const { source } = services;
	const timeline = useTimeline(agent);
	const remote = useView((s) => {
		const device = s.view.tables.workspace.get(lead.workspace)?.device;
		return device ? isRemoteDevice(s.view, device) : false;
	});
	const account = useView((s) =>
		lead.account ? s.view.tables.account.get(lead.account) : undefined
	);
	const [pane, setPane] = useState<HTMLElement | null>(null);
	const [target, setTarget] = useState<JumpTarget | null>(null);
	const find = useFind(agent, timeline, pane, target);
	const title = finishedTitle(agent) ?? t("Sub-agent");
	const kind = useView((s) => agentKind(lead, s.view.tables.account));
	const servers = useView((s) => s.view.tables.model_server);
	const accounts = useView((s) => s.view.tables.account);
	const mark = agentMark(lead, accounts, servers);
	const name = agentKindName(lead, accounts, servers);
	const accountLabel = account ? accountName(account, servers) : kindName(kind);

	const answer = useCallback(
		(attention: AttentionId, value: AttentionAnswer, refs?: readonly InlineRef[]) =>
			source.answer(attention, value, refs),
		[source]
	);
	// A sub-agent it started opens in its own pane.
	const toggleSubAgents = useCallback(
		(subAgent: AgentId) => openAgent(services, subAgent),
		[services]
	);
	const row = useMemo<RowContext>(
		() => ({
			agent,
			vendor: lead.vendor,
			kind,
			name,
			timeline: timeline.store,
			readOnly: true,
			remote,
			answer,
			subAgentsShown: false,
			shownSubAgent: null,
			toggleSubAgents,
		}),
		[agent, lead.vendor, kind, name, timeline.store, remote, answer, toggleSubAgents]
	);

	return (
		<section
			ref={setPane}
			aria-label={title}
			className="flex h-full min-h-0 w-full min-w-0 flex-col bg-background"
		>
			<div className="flex h-10 shrink-0 items-center gap-2.5 border-b border-secondary pr-3 pl-3.5">
				<h1
					title={title}
					className="min-w-16 shrink-[0.2] truncate text-[13.5px] font-[650] tracking-[-0.005em] text-foreground-strong"
				>
					{title}
				</h1>
				<span className="inline-flex h-[22px] shrink-0 items-center gap-[5px] rounded-md border border-track bg-secondary px-[7px] text-[11.5px] whitespace-nowrap text-terminal">
					<VendorIcon kind={mark} />
					{accountLabel}
				</span>
				<span className="inline-flex h-[22px] min-w-12 shrink items-center rounded-md border border-dashed border-input px-[7px] text-[11.5px] whitespace-nowrap text-muted-foreground">
					<span className="truncate">{t("Finished, read only")}</span>
				</span>
			</div>
			<div className="relative flex min-h-0 flex-1 flex-col">
				<ErrorBoundary
					key={`${agent}:timeline`}
					part="timeline"
					className="h-auto min-h-0 flex-1"
				>
					<FindProvider store={find.store}>
						<RowProvider value={row}>
							<TimelineList
								agent={agent}
								timeline={timeline}
								observed={false}
								startedAt={lead.started_at}
								ended={false}
								endedAt={null}
								finished
								onTarget={setTarget}
							/>
						</RowProvider>
					</FindProvider>
				</ErrorBoundary>
				<FindBar find={find} />
			</div>
			<div className="shrink-0 px-7 pb-3.5">
				<SubAgentFinished lead={lead} />
			</div>
		</section>
	);
};
