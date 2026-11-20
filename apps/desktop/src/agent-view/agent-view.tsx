// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One agent's conversation, the same for Claude and Codex: the header, the
// prompts for the ports its processes listen on, the timeline with its find
// bar, and the composer or, for a session in a terminal, the banner that
// says why there's none. A stopped agent gets a Resume banner over its
// composer, or one that opens the agent it moved to, and a local ACP agent
// that needs a sign-in gets its sign-in card there. So does a folder still
// cloning or missing files a copy brought, and a notice while this Mac's
// maxd, which runs it, can't be reached. A file dropped or pasted on an
// answer field goes to the composer. It fills its parent and scrolls on its
// own, and any number can be open side by side.

import { useCallback, useMemo, useState } from "react";
import { useStore } from "zustand";

import { useServices, useUi, useView } from "@/app/services";
import type {
	Agent,
	AgentId,
	AttentionAnswer,
	AttentionId,
	InlineRef,
} from "@maxhq/shared/bindings";

import { ErrorBoundary } from "@/components/error-boundary";
import { Composer } from "@/composer";
import { LocalMaxdNotice } from "@/main-area/local-maxd";
import { createPaneFiles, PaneFilesContext } from "@/composer/pane-files";
import { isRemoteDevice, PortOffers } from "@/ports";
import { TerminalDrawer } from "@/terminal/drawer";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { subAgentLead, useKeepSubAgent } from "@maxhq/shared/agent-view/finished";
import { CloneBanner } from "./clone-banner";
import { FindBar } from "./find/bar";
import { FindProvider } from "@maxhq/shared/agent-view/find-context";
import { useFind } from "./find/use-find";
import { AgentHeader } from "./header";
import type { JumpTarget } from "./jump";
import { OutsideBanner } from "./outside-banner";
import { TimelineQuotes } from "./quotes";
import { MovedBanner, movedAway } from "./moved-banner";
import { canResume, ResumeBanner } from "./resume-banner";
import { needsSignIn } from "@maxhq/shared/settings/acp-agents";
import { slideLead, spotOf } from "@maxhq/shared/ui/store";
import { TimelineList } from "./timeline-list";
import { useAutoOpen } from "./auto-open";
import { useTimeline } from "./use-timeline";

/** One agent's conversation. Fills its parent's height. */
export const AgentView = ({ agent }: { agent: Agent }) => {
	const { source, ui } = useServices();
	const timeline = useTimeline(agent.id);
	useAutoOpen(agent.id, timeline);
	const queued = useStore(timeline.store, (s) => s.queued);
	const device = useView((s) => s.view.tables.workspace.get(agent.workspace)?.device);
	const remote = useView((s) => (device ? isRemoteDevice(s.view, device) : false));
	// So the pane can show the history once the sub-agent finishes.
	useKeepSubAgent(
		agent,
		useView((s) => subAgentLead(s.view.tables.agent, agent))
	);
	const subAgentsShown = useUi((s) => slideLead(s) === agent.id);
	const shownSubAgent = useUi((s) => spotOf(s, agent.id));
	const signIn = useView((s) =>
		needsSignIn(agent, agent.account ? s.view.tables.account.get(agent.account) : undefined)
	);
	const observed = agent.control === "observed";
	// The whole pane takes file drops for the composer, not only its box.
	const [pane, setPane] = useState<HTMLElement | null>(null);
	// And its answer fields hand their files to the composer.
	const [paneFiles] = useState(createPaneFiles);
	// The conversation's scroller, where you pick text to quote.
	const [scroller, setScroller] = useState<HTMLElement | null>(null);
	// What lets find go to an item, once the list mounts.
	const [target, setTarget] = useState<JumpTarget | null>(null);
	const find = useFind(agent.id, timeline, pane, target);

	const answer = useCallback(
		(attention: AttentionId, value: AttentionAnswer, refs?: readonly InlineRef[]) =>
			source.answer(attention, value, refs),
		[source]
	);
	const toggleSubAgents = useCallback(
		(subAgent: AgentId) => {
			const state = ui.getState();
			if (spotOf(state, agent.id) === subAgent) state.closeSubAgents();
			else state.showSubAgent(agent.id, subAgent);
		},
		[ui, agent.id]
	);
	const row = useMemo<RowContext>(
		() => ({
			agent: agent.id,
			vendor: agent.vendor,
			timeline: timeline.store,
			readOnly: observed,
			remote,
			answer,
			subAgentsShown,
			shownSubAgent,
			toggleSubAgents,
		}),
		[
			agent.id,
			agent.vendor,
			timeline.store,
			observed,
			remote,
			answer,
			subAgentsShown,
			shownSubAgent,
			toggleSubAgents,
		]
	);

	return (
		<PaneFilesContext value={paneFiles}>
			<section
				ref={setPane}
				aria-label={agent.title}
				className="flex h-full min-h-0 w-full min-w-0 flex-col bg-background"
			>
				<AgentHeader agent={agent} />
				<PortOffers agent={agent} />
				<div className="relative flex min-h-0 flex-1 flex-col">
					<ErrorBoundary
						key={`${agent.id}:timeline`}
						part="timeline"
						className="h-auto min-h-0 flex-1"
					>
						<FindProvider store={find.store}>
							<RowProvider value={row}>
								<TimelineList
									agent={agent.id}
									timeline={timeline}
									observed={observed}
									startedAt={agent.started_at}
									ended={agent.link.state === "down"}
									endedAt={agent.link.state === "down" ? agent.link.since : null}
									onScroller={setScroller}
									onTarget={setTarget}
								/>
							</RowProvider>
						</FindProvider>
					</ErrorBoundary>
					<FindBar find={find} />
				</div>
				<TimelineQuotes agent={agent} viewport={scroller} store={timeline.store} />
				<CloneBanner workspace={agent.workspace} className="shrink-0 px-7 pb-2" />
				{observed ? <OutsideBanner agent={agent} /> : null}
				{movedAway(agent) ? (
					<MovedBanner agent={agent} />
				) : canResume(agent) || signIn ? (
					<ResumeBanner agent={agent} />
				) : null}
				{device ? (
					<div className="shrink-0 px-7 pb-3.5 empty:hidden">
						<LocalMaxdNotice device={device} className="mx-auto max-w-[744px]" />
					</div>
				) : null}
				{device ? (
					<ErrorBoundary key={`${agent.id}:composer`} part="composer" look="strip">
						<Composer
							agent={agent.id}
							device={device}
							queued={queued}
							timeline={timeline.store}
							dropTarget={pane}
						/>
					</ErrorBoundary>
				) : null}
				{device ? <TerminalDrawer agent={agent.id} device={device} /> : null}
			</section>
		</PaneFilesContext>
	);
};
