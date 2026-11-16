// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Workflow tab of the slide-out: a run picker when the lead has more
// than one run, the run's progress, then one row per node down a lane, with
// nodes the workflow puts aside on a second lane, a group as one row until
// it's opened, and loops that ran drawn in a gutter on the right. A click
// on a node opens its agent, Command splits, and a node with no agent
// opens its popover, as the More button on any row does. The row of the
// selected agent's node shows selected.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Menu } from "@base-ui/react/menu";
import { Ellipsis, Link2, Workflow } from "lucide-react";
import { memo, type MouseEvent, type ReactNode, useMemo, useState } from "react";

import { Elapsed } from "@/agent-view/elapsed";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";
import { holdsCommand } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { Badge } from "@/tasks/activity";
import type {
	AgentId,
	AttemptSpan,
	LoopState,
	NodeDetail,
	NodeShape,
	NodeState,
	RunHeader,
	RunShape,
	WorkflowRun,
	WorkflowRunId,
} from "@maxhq/shared/bindings";
import { formatDuration, plural } from "@maxhq/shared/status/format";
import { badgeTone, glyphOf } from "@maxhq/shared/workflow/glyphs";
import type { RunLoad } from "@maxhq/shared/workflow/use-run";

import { Chevron } from "@/sidebar/agent-row";
import { RightClickMenu } from "@/sidebar/project-menu";

import { NodeGlyph } from "./glyph";
import { LOOP_TONE, NodeCard } from "./node-card";
import {
	groupStatus,
	holdsAgent,
	isReached,
	isSettled,
	type LoopArc,
	loopArcs,
	loopCount,
	nodeAgent,
	openAtFirst,
	type TabRow,
	tabRows,
} from "@maxhq/shared/workflow/layout";

/** A row's height. */
export const ROW = 36;
/** The lane gutter's width, left of each label. */
const GUTTER = 40;
/** The loop gutter's width, right of each row. */
const LOOPS = 24;
// Where each lane runs in the gutter.
const LANE_X = [12, 28] as const;

const center = (row: number) => row * ROW + ROW / 2;

/** What the tab acts through, so it stays a view of what it's given. */
export type WorkflowTabProps = {
	/** The runs to pick from, latest first. The picker shows when there are two or more. */
	readonly runs: readonly WorkflowRun[];
	/** The run shown. */
	readonly run: WorkflowRun;
	/** Its shape, none until the run loads. */
	readonly shape: RunShape | null;
	/** Each node's state, by id. */
	readonly nodes: ReadonlyMap<string, NodeState>;
	readonly loops: readonly LoopState[];
	readonly header: RunHeader | null;
	/** Whether it loaded, and its notice or error. */
	readonly load: RunLoad;
	/** The agent selected, whose node shows selected. */
	readonly selected: AgentId | null;
	/** Whether the header says the run's name, which run mode already does above. */
	readonly named: boolean;
	readonly onPickRun: (run: WorkflowRunId) => void;
	/**
	 * Opens an agent, beside the focused pane when `split`. A row names it
	 * by its `label`, for a sub-agent that finished.
	 */
	readonly onOpenAgent: (agent: AgentId, split: boolean, label?: string) => void;
	/** Opens the run on a canvas, when there's a pane for it. */
	readonly onOpenCanvas?: () => void;
	/** Reads a node's detail lists, when the source offers them. */
	readonly onDetail?: (node: string) => Promise<NodeDetail | null>;
	/** A node row's right-click menu, given the node's agent. Keep it the same function between renders. */
	readonly rowMenu?: (agent: AgentId | null) => ReactNode;
};

const RunPicker = ({
	runs,
	run,
	onPick,
}: {
	runs: readonly WorkflowRun[];
	run: WorkflowRun;
	onPick: (run: WorkflowRunId) => void;
}) => {
	const { t } = useI18n();

	const at = runs.findIndex((r) => r.id === run.id);
	return (
		<Menu.Root>
			<Menu.Trigger
				aria-label={t("Pick a run, {{value1}} of {{length}}", {
					value1: formatNumber(at + 1),
					length: formatNumber(runs.length),
				})}
				className="inline-flex h-[22px] min-w-0 items-center gap-1.5 rounded-md border border-track bg-secondary px-[7px] text-[11.5px] text-secondary-foreground hover:bg-accent data-popup-open:bg-accent"
			>
				<span className="min-w-0 truncate">{run.label}</span>
				<span className="shrink-0 text-muted-foreground tabular-nums">
					{t("{{value2}} of {{length}}", {
						value2: formatNumber(at + 1),
						length: formatNumber(runs.length),
					})}
				</span>
				<Chevron size={9} />
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner side="bottom" align="start" sideOffset={4} className="z-[130]">
					<Menu.Popup className={cn(MENU_POPUP, "w-[260px]")}>
						{runs.map((r) => (
							<Menu.Item
								key={r.id}
								onClick={() => onPick(r.id)}
								className={MENU_ITEM}
							>
								<span className="min-w-0 flex-1 truncate text-[12.5px] font-[550]">
									{r.label}
								</span>
								<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
									{formatNumber(r.progress.done)}/{formatNumber(r.progress.total)}
								</span>
							</Menu.Item>
						))}
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};

/** The lanes behind the glyphs, with a bracket beside each open group. */
const LaneLayer = ({
	rows,
	nodes,
}: {
	rows: readonly TabRow[];
	nodes: ReadonlyMap<string, NodeState>;
}) => {
	useI18n();

	const reached = (row: TabRow) =>
		row.kind === "group"
			? row.members.some((m) => isReached(nodes.get(m.id)?.status ?? "queued"))
			: isReached(nodes.get(row.node.id)?.status ?? "queued");
	const laneOf = (row: TabRow) => (row.kind === "node" ? row.lane : 0);
	const paths: { d: string; solid: boolean }[] = [];
	let main = -1;
	let side = -1;
	for (let i = 0; i <= rows.length; i++) {
		const row = rows[i];
		if (row && laneOf(row) === 1) {
			if (side < 0) side = i;
			continue;
		}
		if (side >= 0) {
			// A branch off the main lane and back, around the rows put aside.
			const [x0, x1] = LANE_X;
			const top = center(side);
			const bottom = center(i - 1);
			let d =
				main >= 0
					? `M ${x0} ${center(main)} C ${x0} ${top - ROW / 2} ${x1} ${top - ROW / 2} ${x1} ${top}`
					: `M ${x1} ${top}`;
			d += ` L ${x1} ${bottom}`;
			if (row)
				d += ` C ${x1} ${bottom + ROW / 2} ${x0} ${bottom + ROW / 2} ${x0} ${center(i)}`;
			paths.push({ d, solid: reached(rows[side]!) });
			side = -1;
		}
		if (!row) break;
		if (main >= 0) {
			paths.push({
				d: `M ${LANE_X[0]} ${center(main)} L ${LANE_X[0]} ${center(i)}`,
				solid: reached(row),
			});
		}
		main = i;
	}
	// A bracket marks each open group's members as running side by side.
	const brackets: string[] = [];
	for (let i = 0; i < rows.length; i++) {
		const row = rows[i]!;
		if (row.kind !== "node" || !row.grouped) continue;
		const first = i;
		while (rows[i + 1]?.kind === "node" && (rows[i + 1] as { grouped: boolean }).grouped) i++;
		brackets.push(`M 7 ${first * ROW + 6} H 4 V ${(i + 1) * ROW - 6} H 7`);
	}
	return (
		<svg
			aria-hidden
			width={GUTTER}
			height={rows.length * ROW}
			className="pointer-events-none absolute top-0 left-0"
		>
			{paths.map((path, i) => (
				<path
					key={i}
					d={path.d}
					fill="none"
					strokeWidth={1.5}
					strokeDasharray={path.solid ? undefined : "3 3"}
					className={path.solid ? "stroke-status-off" : "stroke-track"}
				/>
			))}
			{brackets.map((d, i) => (
				<path
					key={`b${i}`}
					d={d}
					fill="none"
					strokeWidth={1.2}
					className="stroke-muted-foreground"
				/>
			))}
		</svg>
	);
};

/** The loops that ran, as arcs from the row they went back from up to the row they went to. */
const LoopLayer = ({ arcs, height }: { arcs: readonly LoopArc[]; height: number }) => {
	useI18n();
	return (
		<div
			aria-hidden
			className="pointer-events-none absolute top-0 right-0"
			style={{ width: LOOPS }}
		>
			<svg width={LOOPS} height={height} className="absolute top-0 left-0 overflow-visible">
				{arcs.map(({ loop, from, to, tone }, i) => {
					const reach = 10 + (i % 3) * 5;
					const y0 = center(from);
					const y1 = center(to);
					return (
						<g key={`${loop.from}-${loop.to}`} className={LOOP_TONE[tone].stroke}>
							<path
								d={`M 2 ${y0} C ${reach} ${y0} ${reach} ${y1} 2 ${y1}`}
								fill="none"
								strokeWidth={1.4}
							/>
							<path
								d={`M 6 ${y1 - 3} L 2 ${y1} L 6 ${y1 + 3}`}
								fill="none"
								strokeWidth={1.4}
								strokeLinecap="round"
							/>
						</g>
					);
				})}
			</svg>
		</div>
	);
};

/** How long the latest attempt took, or a live clock while it runs. */
const AttemptTime = ({ attempt, busy }: { attempt: AttemptSpan | undefined; busy: boolean }) => {
	useI18n();

	if (!attempt) return null;
	if (attempt.end === null) {
		return busy ? (
			<Elapsed since={attempt.start} className="shrink-0 text-[11px] text-status-busy-text" />
		) : null;
	}
	return (
		<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
			{formatDuration(attempt.end - attempt.start)}
		</span>
	);
};

const ROW_CLASS = "relative flex shrink-0 items-center gap-2 rounded-md pr-1";

/** One node's row. It renders again only when its own state changes. */
export const NodeRow = memo(
	({
		shape,
		node,
		lane,
		selected,
		orchestrator,
		onOpenAgent,
		onDetail,
		rowMenu,
	}: {
		shape: NodeShape;
		node: NodeState;
		lane: 0 | 1;
		selected: boolean;
		orchestrator: AgentId | null;
		onOpenAgent: (agent: AgentId, split: boolean, label?: string) => void;
		onDetail?: (node: string) => Promise<NodeDetail | null>;
		rowMenu?: (agent: AgentId | null) => ReactNode;
	}) => {
		const { t } = useI18n();

		const [open, setOpen] = useState(false);
		const agent = nodeAgent(node, orchestrator);
		const ghost = node.status === "will_skip" || node.status === "skipped";
		const click = (e: MouseEvent) => {
			if (agent) onOpenAgent(agent, holdsCommand(e), shape.label);
			else setOpen(true);
		};
		return (
			<RightClickMenu items={rowMenu ? rowMenu(agent) : null}>
				<li
					aria-current={selected ? "true" : undefined}
					className={cn(
						ROW_CLASS,
						"group/row",
						selected
							? "bg-selected shadow-[inset_0_0_0_1px_var(--selected-ring)]"
							: "hover:bg-accent"
					)}
					style={{ height: ROW }}
				>
					<button
						type="button"
						title={node.note ? `${shape.label}\n${node.note}` : shape.label}
						aria-label={`${shape.label}, ${glyphOf(node.status, node.provisional).label}${agent ? t(", opens its agent") : ""}`}
						onClick={click}
						className="flex h-full min-w-0 flex-1 items-center gap-2 text-left"
						style={{ paddingLeft: GUTTER }}
					>
						<NodeGlyph
							status={node.status}
							provisional={node.provisional}
							className="absolute"
							style={{ left: LANE_X[lane] - 6 }}
						/>
						<span
							className={cn(
								"min-w-0 flex-1 truncate text-xs",
								selected
									? "text-foreground"
									: ghost || node.status === "queued"
										? "text-muted-foreground"
										: "text-terminal"
							)}
						>
							{shape.label}
						</span>
						{node.badges.slice(0, 2).map((badge) => (
							<Badge key={badge.text} tone={badgeTone(badge.tone)}>
								{badge.text}
							</Badge>
						))}
						<AttemptTime attempt={node.attempts.at(-1)} busy={node.status === "busy"} />
						{agent ? (
							<Link2
								aria-hidden
								size={12}
								strokeWidth={2}
								className="shrink-0 text-muted-foreground"
							/>
						) : null}
					</button>
					<Popover open={open} onOpenChange={setOpen}>
						<PopoverTrigger
							aria-label={t("More about {{label}}", { label: shape.label })}
							className="grid size-5 shrink-0 place-items-center rounded-[5px] text-muted-foreground opacity-0 group-hover/row:opacity-100 hover:bg-border hover:text-foreground focus-visible:opacity-100 data-popup-open:opacity-100"
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
					<span aria-hidden className="shrink-0" style={{ width: LOOPS }} />
				</li>
			</RightClickMenu>
		);
	}
);
NodeRow.displayName = "NodeRow";

/** A group's row: its name, how many are done, a dot per member, and a chevron to open it. */
const GroupRow = ({
	row,
	nodes,
	onToggle,
}: {
	row: Extract<TabRow, { kind: "group" }>;
	nodes: ReadonlyMap<string, NodeState>;
	onToggle: (group: string) => void;
}) => {
	const { t } = useI18n();

	const statuses = row.members.map((m) => nodes.get(m.id)?.status ?? "queued");
	const done = statuses.filter(isSettled).length;
	return (
		<li className={cn(ROW_CLASS, "hover:bg-accent")} style={{ height: ROW }}>
			<button
				type="button"
				aria-expanded={row.open}
				aria-label={t("{{label}}, {{done}} of {{value3}} done", {
					label: row.label,
					done: formatNumber(done),
					value3: plural(row.members.length, "node"),
				})}
				onClick={() => onToggle(row.id)}
				className="flex h-full min-w-0 flex-1 items-center gap-2 text-left"
				style={{ paddingLeft: GUTTER }}
			>
				<NodeGlyph
					status={groupStatus(statuses)}
					provisional={false}
					className="absolute"
					style={{ left: LANE_X[0] - 6 }}
				/>
				<span className="min-w-0 truncate text-xs text-terminal">{row.label}</span>
				<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
					{formatNumber(done)}/{formatNumber(row.members.length)}
				</span>
				<span className="flex min-w-0 flex-1 items-center gap-[3px] overflow-hidden">
					{row.members.map((member, i) => (
						<NodeGlyph
							key={member.id}
							status={statuses[i]!}
							provisional={nodes.get(member.id)?.provisional ?? false}
							className="size-[9px]"
						/>
					))}
				</span>
				<Chevron closed={!row.open} />
			</button>
			<span aria-hidden className="shrink-0" style={{ width: LOOPS }} />
		</li>
	);
};

/** The rows, lanes and loops of one run. */
const RunRows = ({
	shape,
	nodes,
	loops,
	selected,
	orchestrator,
	onOpenAgent,
	onDetail,
	rowMenu,
}: {
	shape: RunShape;
	nodes: ReadonlyMap<string, NodeState>;
	loops: readonly LoopState[];
	selected: AgentId | null;
	orchestrator: AgentId | null;
	onOpenAgent: (agent: AgentId, split: boolean, label?: string) => void;
	onDetail?: (node: string) => Promise<NodeDetail | null>;
	rowMenu?: (agent: AgentId | null) => ReactNode;
}) => {
	const { t } = useI18n();

	const [open, setOpen] = useState(() => openAtFirst(shape));
	const rows = useMemo(
		() => tabRows(shape, (id) => nodes.get(id)?.status, open),
		[shape, nodes, open]
	);
	const arcs = useMemo(() => loopArcs(rows, loops), [rows, loops]);
	const toggle = (group: string) =>
		setOpen((was) => {
			const next = new Set(was);
			if (!next.delete(group)) next.add(group);
			return next;
		});
	return (
		<div className="relative animate-reveal-down">
			<LaneLayer rows={rows} nodes={nodes} />
			<ul aria-label={t("Nodes")} className="flex flex-col">
				{rows.map((row) => {
					if (row.kind === "group") {
						return (
							<GroupRow
								key={`g:${row.id}`}
								row={row}
								nodes={nodes}
								onToggle={toggle}
							/>
						);
					}
					const node = nodes.get(row.node.id);
					if (!node) return null;
					return (
						<NodeRow
							key={row.node.id}
							shape={row.node}
							node={node}
							lane={row.lane}
							selected={holdsAgent(node, selected)}
							orchestrator={orchestrator}
							onOpenAgent={onOpenAgent}
							onDetail={onDetail}
							rowMenu={rowMenu}
						/>
					);
				})}
			</ul>
			{arcs.length > 0 ? <LoopLayer arcs={arcs} height={rows.length * ROW} /> : null}
			{arcs.map(({ loop, from, to, tone }) => (
				<span
					key={`${loop.from}-${loop.to}`}
					role="img"
					aria-label={t("{{value1}} went round {{value2}}", {
						value1: loop.label ?? t("Loop"),
						value2: loopCount(loop),
					})}
					className={cn(
						"absolute right-0 w-6 text-center text-[9.5px] font-[650] tabular-nums",
						LOOP_TONE[tone].text
					)}
					style={{ top: (center(from) + center(to)) / 2 - 7 }}
				>
					{loopCount(loop)}
				</span>
			))}
		</div>
	);
};

/** The Workflow tab for one run. */
export const WorkflowTab = ({
	runs,
	run,
	shape,
	nodes,
	loops,
	header,
	load,
	selected,
	named,
	onPickRun,
	onOpenAgent,
	onOpenCanvas,
	onDetail,
	rowMenu,
}: WorkflowTabProps) => {
	const { t } = useI18n();

	const progress = header?.progress ?? run.progress;
	return (
		<div className="flex min-h-0 flex-col gap-1.5">
			<div className="flex flex-col gap-1 px-1.5 pb-1">
				{runs.length > 1 ? <RunPicker runs={runs} run={run} onPick={onPickRun} /> : null}
				<div className="flex items-center gap-2">
					{named ? (
						<span
							title={run.label}
							className="min-w-0 flex-1 truncate text-xs font-semibold text-foreground"
						>
							{header?.label ?? run.label}
						</span>
					) : (
						<span className="flex-1" />
					)}
					<span
						role="img"
						aria-label={t("{{done}} of {{total}} done", {
							done: formatNumber(progress.done),
							total: formatNumber(progress.total),
						})}
						className="shrink-0 text-[11px] text-secondary-foreground tabular-nums"
					>
						{formatNumber(progress.done)}/{formatNumber(progress.total)}
					</span>
					{onOpenCanvas ? (
						<button
							type="button"
							onClick={onOpenCanvas}
							className="inline-flex h-[22px] shrink-0 items-center gap-1 rounded-md px-1.5 text-[11.5px] text-secondary-foreground hover:bg-accent hover:text-foreground"
						>
							<Workflow aria-hidden size={12} strokeWidth={2} />
							{t("Open canvas")}
						</button>
					) : null}
				</div>
				{header?.lines?.length ? (
					<div className="flex flex-wrap gap-1">
						{header.lines.map((line) => (
							<Badge key={line.text} tone={badgeTone(line.tone)}>
								{line.text}
							</Badge>
						))}
					</div>
				) : null}
				{load.notice ? (
					<p
						role="status"
						className={cn("text-[11px]", LOOP_TONE[load.notice.tone].text)}
					>
						{load.notice.text}
					</p>
				) : null}
			</div>
			{shape ? (
				<RunRows
					// A new run starts from its own folds and reveal. A live run's
					// shape grows as it goes, and keeps its folds.
					key={run.id}
					shape={shape}
					nodes={nodes}
					loops={loops}
					selected={selected}
					orchestrator={run.orchestrators.at(-1) ?? null}
					onOpenAgent={onOpenAgent}
					onDetail={onDetail}
					rowMenu={rowMenu}
				/>
			) : load.status === "failed" ? (
				<p role="alert" className="px-1.5 text-[11.5px] text-status-error-text">
					{t("Couldn't open the run{{value2}}", {
						value2: load.error ? `: ${load.error}` : "",
					})}
				</p>
			) : (
				<p className="px-1.5 text-[11.5px] text-muted-foreground">{t("Loading the run")}</p>
			)}
		</div>
	);
};
