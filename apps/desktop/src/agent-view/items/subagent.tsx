// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A sub-agent the agent started: what it was asked, how many tool calls it
// has made, how long it has run and the step it's on, and a button that
// shows it in the side panel. Its report opens below.

import { ChevronDown, ChevronRight, CircleCheck, CircleX, PanelLeft } from "lucide-react";
import { useCallback } from "react";

import type { AgentId, TimelineItem } from "@maxhq/shared/bindings";
import { sentence } from "@maxhq/shared/status/format";
import { cn } from "@/lib/utils";

import { useRow } from "@maxhq/shared/agent-view/context";
import { Elapsed } from "../elapsed";
import { isOpen } from "@maxhq/shared/agent-view/format";
import {
	endedLine,
	everything,
	runningLine,
	useWork,
	type Work,
} from "@maxhq/shared/agent-view/subagent";
import { Spinner } from "../icons";
import { useFold } from "@maxhq/shared/agent-view/find-context";
import { Markdown } from "../markdown";
import { useTimeline } from "../use-timeline";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const Meta = ({ item, work }: { item: ItemOf<"subagent">; work: Work }) => {
	useI18n();
	if (!isOpen(item.status)) {
		return <span className="min-w-0 truncate">{endedLine(item, work)}</span>;
	}
	const { words, since } = runningLine(item.started_at, work);
	const text = (
		<>
			{words}
			{since !== null ? <Elapsed since={since} /> : null}
		</>
	);
	if (work.now === null) return <span className="min-w-0 truncate">{text}</span>;
	return (
		<>
			<span className="shrink-0 whitespace-nowrap">
				{text}
				{translate(". Now:")}
			</span>
			<span
				title={work.now}
				className="min-w-0 truncate font-mono text-[11px] text-secondary-foreground"
			>
				{work.now}
			</span>
		</>
	);
};

// A sub-agent with a timeline of its own, which the card opens while shown.
const OwnMeta = ({ agent, item }: { agent: AgentId; item: ItemOf<"subagent"> }) => {
	useI18n();
	const timeline = useTimeline(agent);
	const work = useWork(timeline.store, everything, isOpen(item.status), true);
	return <Meta item={item} work={work} />;
};

// A sub-agent whose steps sit under its card in the lead's timeline.
const NestedMeta = ({ item }: { item: ItemOf<"subagent"> }) => {
	useI18n();
	const { timeline } = useRow();
	const mine = useCallback((other: TimelineItem) => other.parent === item.id, [item.id]);
	return <Meta item={item} work={useWork(timeline, mine, isOpen(item.status), false)} />;
};

/** A sub-agent card. */
export const SubAgentCard = ({ item }: { item: ItemOf<"subagent"> }) => {
	useI18n();
	const { subAgentsShown, shownSubAgent, toggleSubAgents } = useRow();
	// Only the card whose row the panel points out reads as shown.
	const subAgent = item.subagent;
	const shown = subAgentsShown && subAgent !== null && shownSubAgent === subAgent;
	// While find's active match is in it, the row shows its whole description and its report.
	const [open, setOpen, held] = useFold(`open:${item.id}`, item.id);
	const running = isOpen(item.status);
	const hasResult = Boolean(item.result?.trim());

	const status = running ? (
		<Spinner size={10} className="mx-0.5" />
	) : item.status === "failed" ? (
		<CircleX size={14} strokeWidth={1.9} aria-hidden className="text-destructive" />
	) : (
		<CircleCheck size={14} strokeWidth={1.9} aria-hidden className="text-status-ready" />
	);
	const summary = (
		<>
			{status}
			<span className="flex min-w-0 flex-1 flex-col gap-0.5">
				<span
					className={cn(
						"text-[12.5px] text-foreground",
						held ? "break-words" : "truncate"
					)}
				>
					<span className="font-semibold">
						{item.agent_type ? sentence(item.agent_type) : translate("Task")}:
					</span>{" "}
					<span data-find="plain">{item.description}</span>
				</span>
				<span className="flex min-w-0 items-center gap-1.5 text-[11.5px] text-muted-foreground tabular-nums">
					{item.subagent ? (
						<OwnMeta agent={item.subagent} item={item} />
					) : (
						<NestedMeta item={item} />
					)}
				</span>
			</span>
			{hasResult ? (
				open ? (
					<ChevronDown
						size={10}
						strokeWidth={2.4}
						aria-hidden
						className="shrink-0 text-muted-foreground"
					/>
				) : (
					<ChevronRight
						size={10}
						strokeWidth={2.4}
						aria-hidden
						className="shrink-0 text-muted-foreground"
					/>
				)
			) : null}
		</>
	);

	return (
		<div className="flex flex-col gap-1.5">
			<div className="flex items-center gap-2.5 rounded-lg border border-selected-ring bg-subpanel py-[9px] pr-2.5 pl-3">
				{hasResult ? (
					<button
						type="button"
						aria-expanded={open}
						aria-label={translate("{{value1}} the report from {{description}}", {
							value1: open ? translate("Hide") : translate("Show"),
							description: item.description,
						})}
						onClick={() => setOpen(!open)}
						className="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5 text-left"
					>
						{summary}
					</button>
				) : (
					<div className="flex min-w-0 flex-1 items-center gap-2.5">{summary}</div>
				)}
				{subAgent ? (
					<button
						type="button"
						aria-pressed={shown}
						onClick={() => toggleSubAgents(subAgent)}
						className={cn(
							"inline-flex h-[26px] shrink-0 cursor-pointer items-center gap-1.5 rounded-md border px-2.5 text-[12px]",
							shown
								? "border-selected-ring bg-selected text-foreground-strong"
								: "border-input bg-secondary text-foreground hover:bg-accent"
						)}
					>
						<PanelLeft size={12} strokeWidth={1.9} aria-hidden />
						{shown ? translate("Shown in panel") : translate("Show in panel")}
					</button>
				) : null}
			</div>
			{open && hasResult ? (
				<div className="ml-[19px] border-l-2 border-track pl-4">
					<Markdown
						text={item.result ?? ""}
						className="text-[12.5px] text-secondary-foreground"
					/>
				</div>
			) : null}
		</div>
	);
};
