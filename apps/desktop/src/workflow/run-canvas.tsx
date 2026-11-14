// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One run on a canvas you pan and zoom: a card per node where ELK put it,
// group boxes around their members, edges down from what a node needs,
// and loops on a lane to the right. Every card reads its own node, so a
// status change draws one card again, and the layout changes only with the
// shape. A click on a card opens its agent, Command splits, and a card
// with no agent opens its popover, as the More button on any card does.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import "@xyflow/react/dist/base.css";

import {
	type Node,
	type NodeProps,
	type NodeTypes,
	ReactFlow,
	ReactFlowProvider,
	ViewportPortal,
} from "@xyflow/react";
import { Ellipsis, Link2 } from "lucide-react";
import { createContext, memo, type MouseEvent, useContext, useMemo, useState } from "react";

import { useUi } from "@/app/services";
import { useTheme } from "@/app/theme";
import { useNode, useRunValue } from "@/app/use-run";
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";
import { holdsCommand } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { Badge } from "@/tasks/activity";
import type {
	AgentId,
	LoopState,
	NodeDetail,
	NodeShape,
	RunKey,
	RunShape,
} from "@maxhq/shared/bindings";
import { formatDuration } from "@maxhq/shared/status/format";
import {
	type CanvasEdge,
	type CanvasLayout,
	CARD,
	type GroupBox,
	type LoopRoute,
	loopRoutes,
	pathThrough,
} from "@maxhq/shared/workflow/canvas";
import { badgeTone, glyphOf } from "@maxhq/shared/workflow/glyphs";
import {
	holdsAgent,
	isReached,
	isSettled,
	loopCount,
	nodeAgent,
} from "@maxhq/shared/workflow/layout";

import { NodeGlyph } from "./glyph";
import { LOOP_TONE, NodeCard } from "./node-card";

/** What every card acts through. It stays the same while the run is shown. */
type CanvasActions = {
	readonly run: RunKey;
	readonly orchestrator: AgentId | null;
	readonly onOpenAgent: (agent: AgentId, split: boolean) => void;
	readonly onDetail?: (node: string) => Promise<NodeDetail | null>;
};

const Actions = createContext<CanvasActions | null>(null);

const useActions = (): CanvasActions => {
	const actions = useContext(Actions);
	if (!actions) throw new Error("A canvas card needs its canvas");
	return actions;
};

type CardNode = Node<{ shape: NodeShape }, "card">;
type GroupNode = Node<{ group: GroupBox }, "group">;

/**
 * One node's card. It renders again only when its own state changes. React
 * Flow lets clicks through a node that can't be dragged or selected, so the
 * card takes them back.
 */
const Card = memo(({ data: { shape } }: NodeProps<CardNode>) => {
	const { t } = useI18n();

	const { run, orchestrator, onOpenAgent, onDetail } = useActions();
	const node = useNode(run, shape.id);
	const selected = useUi((s) => (node ? holdsAgent(node, s.selectedAgent) : false));
	const [open, setOpen] = useState(false);
	if (!node) return null;
	const agent = nodeAgent(node, orchestrator);
	const glyph = glyphOf(node.status, node.provisional);
	const last = node.attempts.at(-1);
	const quiet =
		node.status === "omitted" || node.status === "will_skip" || node.status === "skipped";
	const click = (e: MouseEvent) => {
		if (agent) onOpenAgent(agent, holdsCommand(e));
		else setOpen(true);
	};
	return (
		<div
			aria-current={selected ? "true" : undefined}
			className={cn(
				"group/card pointer-events-auto relative flex rounded-lg border bg-subpanel",
				selected
					? "border-transparent bg-selected shadow-[inset_0_0_0_1px_var(--selected-ring)]"
					: "border-track hover:bg-accent",
				quiet && "opacity-55"
			)}
			style={CARD}
		>
			<button
				type="button"
				title={node.note ? `${shape.label}\n${node.note}` : shape.label}
				aria-label={`${shape.label}, ${glyph.label}${agent ? t(", opens its agent") : ""}`}
				onClick={click}
				className="nodrag nopan flex min-w-0 flex-1 flex-col justify-center gap-1 pr-1 pl-2.5 text-left"
			>
				<span className="flex min-w-0 items-center gap-2">
					<NodeGlyph status={node.status} provisional={node.provisional} />
					<span
						className={cn(
							"min-w-0 flex-1 truncate text-xs",
							selected ? "text-foreground" : "text-terminal"
						)}
					>
						{shape.label}
					</span>
					{agent ? (
						<Link2
							aria-hidden
							size={12}
							strokeWidth={2}
							className="shrink-0 text-muted-foreground"
						/>
					) : null}
				</span>
				<span className="flex min-w-0 items-center gap-1.5 pl-5 text-[11px] text-muted-foreground">
					<span className="min-w-0 truncate">{node.note ?? glyph.label}</span>
					{node.badges.slice(0, 1).map((badge) => (
						<Badge key={badge.text} tone={badgeTone(badge.tone)}>
							{badge.text}
						</Badge>
					))}
					{last && last.end !== null ? (
						<span className="ml-auto shrink-0 tabular-nums">
							{formatDuration(last.end - last.start)}
						</span>
					) : null}
				</span>
			</button>
			<Popover open={open} onOpenChange={setOpen}>
				<PopoverTrigger
					aria-label={t("More about {{label}}", { label: shape.label })}
					className="nodrag nopan mt-1.5 mr-1 grid size-5 shrink-0 place-items-center rounded-[5px] text-muted-foreground opacity-0 group-hover/card:opacity-100 hover:bg-border hover:text-foreground focus-visible:opacity-100 data-popup-open:opacity-100"
				>
					<Ellipsis aria-hidden size={12} strokeWidth={2.2} />
				</PopoverTrigger>
				<PopoverPopup side="right" align="start" sideOffset={8}>
					<NodeCard
						shape={shape}
						node={node}
						onOpenAgent={(id, split) => {
							setOpen(false);
							onOpenAgent(id, split);
						}}
						onDetail={onDetail}
					/>
				</PopoverPopup>
			</Popover>
		</div>
	);
});
Card.displayName = "Card";

/** A group's box: its name and how many of its members are done. */
const Group = memo(({ data: { group } }: NodeProps<GroupNode>) => {
	const { t } = useI18n();

	const { run } = useActions();
	const done = useRunValue(
		run,
		(s) => group.members.filter((id) => isSettled(s.nodes.get(id)?.status ?? "queued")).length
	);
	return (
		<div
			role="group"
			aria-label={t("{{label}}, {{done}} of {{length}} done", {
				label: group.label,
				done: formatNumber(done),
				length: formatNumber(group.members.length),
			})}
			className="rounded-xl border border-dashed border-track bg-secondary/30"
			style={{ width: group.width, height: group.height }}
		>
			<span className="flex h-7 items-center gap-1.5 px-3 text-[11px] text-muted-foreground">
				<span className="font-semibold text-secondary-foreground">{group.label}</span>
				<span className="tabular-nums">
					{formatNumber(done)}/{formatNumber(group.members.length)}
				</span>
			</span>
		</div>
	);
});
Group.displayName = "Group";

const NODE_TYPES: NodeTypes = { card: Card, group: Group };

/** An edge, solid once the node it leads to is reached. */
const Edge = memo(({ edge }: { edge: CanvasEdge }) => {
	useI18n();

	const { run } = useActions();
	const reached = useRunValue(run, (s) => isReached(s.nodes.get(edge.to)?.status ?? "queued"));
	return (
		<path
			d={pathThrough(edge.points)}
			fill="none"
			strokeWidth={1.5}
			strokeDasharray={reached ? undefined : "3 3"}
			markerEnd="url(#canvas-arrow)"
			className={reached ? "stroke-status-off" : "stroke-track"}
		/>
	);
});
Edge.displayName = "Edge";

/** A loop on its lane, with how many times it went round. */
const Loop = ({ route }: { route: LoopRoute }) => {
	const { t } = useI18n();

	const tone = LOOP_TONE[route.tone];
	const { loop, label } = route;
	const end = route.points.at(-1)!;
	return (
		<g className={tone.stroke}>
			<path
				d={pathThrough(route.points)}
				fill="none"
				strokeWidth={1.4}
				strokeDasharray={route.possible ? "2 3" : undefined}
			/>
			<path
				d={`M ${end.x + 5} ${end.y - 3} L ${end.x} ${end.y} L ${end.x + 5} ${end.y + 3}`}
				fill="none"
				strokeWidth={1.4}
				strokeLinecap="round"
			/>
			<g transform={`translate(${label.x} ${label.y})`}>
				<rect x={-13} y={-8} width={26} height={16} rx={8} className="fill-background" />
				<text
					role="img"
					aria-label={
						route.possible
							? t("{{value1}} can go back here", { value1: loop.label ?? t("Loop") })
							: t("{{value1}} went round {{value2}}", {
									value1: loop.label ?? t("Loop"),
									value2: loopCount(loop),
								})
					}
					textAnchor="middle"
					y={3.5}
					fontSize={10}
					fontWeight={650}
					stroke="none"
					className={cn("fill-current tabular-nums", tone.text)}
				>
					{route.possible ? "0" : loopCount(loop)}
				</text>
			</g>
		</g>
	);
};

/** The edges and loops, drawn in the canvas's own coordinates. */
const Lines = ({ layout, loops }: { layout: CanvasLayout; loops: readonly LoopRoute[] }) => {
	useI18n();
	return (
		<ViewportPortal>
			<svg
				width={1}
				height={1}
				className="pointer-events-none absolute top-0 left-0 overflow-visible"
			>
				<defs>
					<marker
						id="canvas-arrow"
						viewBox="0 0 8 8"
						refX={7}
						refY={4}
						markerWidth={7}
						markerHeight={7}
						orient="auto"
					>
						<path
							d="M 1 1 L 7 4 L 1 7"
							fill="none"
							strokeWidth={1.4}
							className="stroke-status-off"
						/>
					</marker>
				</defs>
				<g aria-hidden>
					{layout.edges.map((edge) => (
						<Edge key={edge.id} edge={edge} />
					))}
				</g>
				{loops.map((route) => (
					<Loop key={`${route.loop.from}-${route.loop.to}`} route={route} />
				))}
			</svg>
		</ViewportPortal>
	);
};

const nodesOf = (shape: RunShape, layout: CanvasLayout): Node[] => [
	...layout.groups.map((group): GroupNode => ({
		id: `group:${group.id}`,
		type: "group",
		position: { x: group.x, y: group.y },
		width: group.width,
		height: group.height,
		data: { group },
		zIndex: 0,
	})),
	...shape.nodes.flatMap((node): CardNode[] => {
		const box = layout.nodes.get(node.id);
		return box
			? [
					{
						id: node.id,
						type: "card",
						position: { x: box.x, y: box.y },
						width: box.width,
						height: box.height,
						data: { shape: node },
						zIndex: 1,
					},
				]
			: [];
	}),
];

export type RunCanvasProps = CanvasActions & {
	readonly shape: RunShape;
	readonly layout: CanvasLayout;
	readonly loops: readonly LoopState[];
	/** Whether to draw the loops that haven't run too. */
	readonly possible: boolean;
};

/** The canvas for one run, laid out. */
export const RunCanvas = ({
	run,
	orchestrator,
	onOpenAgent,
	onDetail,
	shape,
	layout,
	loops,
	possible,
}: RunCanvasProps) => {
	const { t } = useI18n();

	const actions = useMemo(
		() => ({ run, orchestrator, onOpenAgent, onDetail }),
		[run, orchestrator, onOpenAgent, onDetail]
	);
	const nodes = useMemo(() => nodesOf(shape, layout), [shape, layout]);
	const routes = useMemo(() => loopRoutes(layout, loops, possible), [layout, loops, possible]);
	const theme = useTheme();
	return (
		<Actions.Provider value={actions}>
			<ReactFlowProvider>
				<ReactFlow
					// A new layout fits the view again.
					key={layout.hash}
					aria-label={t("Workflow canvas")}
					colorMode={theme}
					nodes={nodes}
					nodeTypes={NODE_TYPES}
					nodesDraggable={false}
					nodesConnectable={false}
					nodesFocusable={false}
					elementsSelectable={false}
					zoomOnDoubleClick={false}
					minZoom={0.25}
					maxZoom={1.5}
					fitView
					fitViewOptions={{ padding: 0.12, maxZoom: 1 }}
					className="bg-background"
				>
					<Lines layout={layout} loops={routes} />
				</ReactFlow>
			</ReactFlowProvider>
		</Actions.Provider>
	);
};
