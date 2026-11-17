// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One run over time: a row per node and a bar per attempt, colored by how
// it ended, with the time it waits on you hatched amber. A bar with an
// agent opens it. What the run doesn't record, the timeline says below.

import { useI18n } from "@maxhq/shared/i18n";

import { scaleTime } from "d3-scale";
import { type KeyboardEvent, type MouseEvent, useEffect, useMemo, useRef, useState } from "react";

import { holdsCommand } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import type { AgentId, NodeState, RunShape, Timestamp } from "@maxhq/shared/bindings";
import { formatClock } from "@maxhq/shared/status/format";
import { barLabel, timeline } from "@maxhq/shared/workflow/canvas";
import { glyphOf } from "@maxhq/shared/workflow/glyphs";

import { GLYPH_COLOR } from "./glyph";

const ROW = 28;
const BAR = 14;
const LABELS = 168;
const AXIS = 22;
// The width before the pane reports its own, as under tests.
const WIDE = 640;

/** The element's width, kept up to date. */
const useWidth = () => {
	const ref = useRef<HTMLDivElement>(null);
	const [width, setWidth] = useState(0);
	useEffect(() => {
		const element = ref.current;
		if (!element) return;
		const observer = new ResizeObserver(([entry]) => {
			if (entry) setWidth(entry.contentRect.width);
		});
		observer.observe(element);
		return () => observer.disconnect();
	}, []);
	return [ref, width || WIDE] as const;
};

export type RunTimelineProps = {
	readonly shape: RunShape;
	readonly nodes: ReadonlyMap<string, NodeState>;
	readonly now: Timestamp;
	readonly onOpenAgent: (agent: AgentId, split: boolean) => void;
};

/** The run's attempts on a time axis. */
export const RunTimeline = ({ shape, nodes, now, onOpenAgent }: RunTimelineProps) => {
	const { t } = useI18n();

	const line = useMemo(() => timeline(shape, nodes, now), [shape, nodes, now]);
	const [ref, width] = useWidth();
	const chart = Math.max(120, width - LABELS - 16);
	const x = useMemo(
		() =>
			scaleTime()
				.domain([line.start, Math.max(line.end, line.start + 60_000)])
				.range([0, chart])
				.nice(),
		[line.start, line.end, chart]
	);
	const ticks = x.ticks(Math.max(2, Math.floor(chart / 110)));
	const height = AXIS + line.rows.length * ROW;
	const open = (agent: AgentId | null, e: MouseEvent | KeyboardEvent) => {
		if (agent) onOpenAgent(agent, holdsCommand(e));
	};

	return (
		<div ref={ref} className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto p-3">
			<svg
				role="list"
				aria-label={t("Attempts over time")}
				width={LABELS + chart}
				height={height}
				className="shrink-0"
			>
				<defs>
					<pattern
						id="timeline-wait"
						width={6}
						height={6}
						patternUnits="userSpaceOnUse"
						patternTransform="rotate(45)"
					>
						<rect width={6} height={6} className="fill-status-busy/25" />
						<path d="M 0 0 V 6" strokeWidth={2.5} className="stroke-status-busy" />
					</pattern>
				</defs>
				<g aria-hidden transform={`translate(${LABELS} 0)`}>
					{ticks.map((tick) => (
						<g key={tick.getTime()} transform={`translate(${x(tick)} 0)`}>
							<line
								y1={AXIS - 4}
								y2={height}
								className="stroke-track"
								strokeWidth={1}
							/>
							<text
								y={12}
								textAnchor="middle"
								fontSize={10.5}
								className="fill-muted-foreground tabular-nums"
							>
								{formatClock(tick.getTime())}
							</text>
						</g>
					))}
				</g>
				{line.rows.map((row, r) => {
					const y = AXIS + r * ROW;
					return (
						<g key={row.node.id} role="listitem" aria-label={row.node.label}>
							<text
								x={LABELS - 10}
								y={y + ROW / 2 + 4}
								textAnchor="end"
								fontSize={12}
								className="fill-terminal"
							>
								{row.node.label.length > 22
									? `${row.node.label.slice(0, 21)}…`
									: row.node.label}
							</text>
							{row.bars.map((bar, i) => {
								const left = LABELS + x(bar.start);
								const barWidth = Math.max(3, x(bar.end) - x(bar.start));
								const top = y + (ROW - BAR) / 2;
								const color = GLYPH_COLOR[glyphOf(bar.status).color];
								// The part before the wait, all of it when it doesn't wait.
								const solid =
									bar.waitFrom === null
										? barWidth
										: Math.min(
												barWidth,
												Math.max(0, x(bar.waitFrom) - x(bar.start))
											);
								return (
									<g
										key={bar.start}
										role={bar.agent ? "button" : "img"}
										tabIndex={bar.agent ? 0 : undefined}
										aria-label={barLabel(bar, i)}
										onClick={(e) => open(bar.agent, e)}
										onKeyDown={(e) => {
											if (e.key === "Enter" || e.key === " ") {
												e.preventDefault();
												open(bar.agent, e);
											}
										}}
										className={cn(
											color,
											bar.agent &&
												"cursor-pointer outline-none focus-visible:opacity-80"
										)}
									>
										<title>{barLabel(bar, i)}</title>
										{solid > 0 ? (
											<rect
												x={left}
												y={top}
												width={solid}
												height={BAR}
												rx={3}
												className="fill-current"
												fillOpacity={bar.running ? 1 : 0.75}
											/>
										) : null}
										{bar.waitFrom === null ? null : (
											<rect
												x={left + solid}
												y={top}
												width={barWidth - solid}
												height={BAR}
												rx={3}
												fill="url(#timeline-wait)"
											/>
										)}
										{bar.running && bar.waitFrom === null ? (
											<rect
												x={left + barWidth - 2}
												y={top}
												width={2}
												height={BAR}
												className="fill-foreground"
											/>
										) : null}
									</g>
								);
							})}
						</g>
					);
				})}
			</svg>
			{line.rows.every((row) => row.bars.length === 0) ? (
				<p className="text-[11.5px] text-muted-foreground">{t("Nothing has run yet.")}</p>
			) : null}
			{line.missing.map((text) => (
				<p key={text} role="note" className="text-[11.5px] text-status-busy-text">
					{text}.
				</p>
			))}
		</div>
	);
};
