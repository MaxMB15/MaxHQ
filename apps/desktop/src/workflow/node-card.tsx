// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a node's popover shows, on the Workflow tab and the canvas alike,
// and the colors a loop takes from its tone.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useEffect, useState } from "react";

import { PopoverTitle } from "@/components/ui/popover";
import { OutLink } from "@/pulls/look";
import { Badge } from "@/tasks/activity";
import type { AgentId, NodeDetail, NodeShape, NodeState } from "@maxhq/shared/bindings";
import { formatDuration } from "@maxhq/shared/status/format";
import { badgeTone, glyphOf } from "@maxhq/shared/workflow/glyphs";

/** How each tone colors a loop and its counter. */
export const LOOP_TONE = {
	info: { stroke: "stroke-primary", text: "text-primary" },
	neutral: { stroke: "stroke-muted-foreground", text: "text-muted-foreground" },
	good: { stroke: "stroke-status-ready", text: "text-status-ready-text" },
	warn: { stroke: "stroke-status-busy", text: "text-status-busy-text" },
	bad: { stroke: "stroke-status-error", text: "text-status-error-text" },
} as const;

const DetailLists = ({
	node,
	onDetail,
}: {
	node: string;
	onDetail: (node: string) => Promise<NodeDetail | null>;
}) => {
	useI18n();

	const [detail, setDetail] = useState<NodeDetail | null>(null);
	useEffect(() => {
		let live = true;
		onDetail(node).then(
			(got) => {
				if (live) setDetail(got);
			},
			(error: unknown) => console.warn("Couldn't read the node's detail", error)
		);
		return () => {
			live = false;
		};
	}, [node, onDetail]);
	if (!detail) return null;
	return (
		<>
			{detail.lists.map((list) => (
				<section key={list.label} className="flex flex-col gap-0.5">
					<h4 className="text-[11px] font-semibold text-muted-foreground">
						{list.label}
					</h4>
					<ul className="flex flex-col text-[11.5px] text-secondary-foreground">
						{list.items.map((item) => (
							<li key={item} className="truncate" title={item}>
								{item}
							</li>
						))}
					</ul>
				</section>
			))}
		</>
	);
};

/** A node's popover, on the Workflow tab and the canvas: its status, note, badges, every attempt with its agent, links and detail. */
export const NodeCard = ({
	shape,
	node,
	onOpenAgent,
	onDetail,
}: {
	shape: NodeShape;
	node: NodeState;
	onOpenAgent: (agent: AgentId, split: boolean) => void;
	onDetail?: (node: string) => Promise<NodeDetail | null>;
}) => {
	const { t } = useI18n();
	return (
		<div className="flex w-[260px] flex-col gap-2 p-3">
			<PopoverTitle className="text-[12.5px] leading-snug">{shape.label}</PopoverTitle>
			<p className="text-[11.5px] text-secondary-foreground">
				{glyphOf(node.status, node.provisional).label}
				{node.quiet ? t(", quiet for a while") : ""}
			</p>
			{node.note ? <p className="text-[11.5px] text-muted-foreground">{node.note}</p> : null}
			{node.badges.length > 0 ? (
				<div className="flex flex-wrap gap-1">
					{node.badges.map((badge) => (
						<Badge key={badge.text} tone={badgeTone(badge.tone)}>
							{badge.text}
						</Badge>
					))}
				</div>
			) : null}
			{node.attempts.length > 0 ? (
				<ol aria-label={t("Attempts")} className="flex flex-col gap-0.5">
					{node.attempts.map((attempt, i) => (
						<li
							key={attempt.start}
							className="flex h-6 items-center gap-2 text-[11.5px]"
						>
							<span className="text-secondary-foreground">
								{t("Try {{value2}}", { value2: formatNumber(i + 1) })}
							</span>
							<span className="min-w-0 flex-1 truncate text-muted-foreground">
								{attempt.end_status
									? glyphOf(attempt.end_status).label
									: t("Running")}
								{attempt.end !== null
									? `, ${formatDuration(attempt.end - attempt.start)}`
									: ""}
							</span>
							{attempt.agent ? (
								<button
									type="button"
									onClick={() => onOpenAgent(attempt.agent!, false)}
									className="shrink-0 rounded-[5px] px-1.5 text-primary hover:bg-accent"
								>
									{t("Open")}
								</button>
							) : null}
						</li>
					))}
				</ol>
			) : null}
			{node.links.map((link) => (
				<OutLink key={link.url} href={link.url}>
					{link.label}
				</OutLink>
			))}
			{onDetail ? <DetailLists node={node.id} onDetail={onDetail} /> : null}
		</div>
	);
};
