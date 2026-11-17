// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone's workflow screens work out before they draw: the colors
// a status and a tone take, a run's row and chip, the menus, what a node
// says, where loops show, and the timeline's time axis, which the desktop
// gets from d3.

import { t, formatDate, formatNumber } from "@maxhq/shared/i18n";

import type {
	AgentId,
	LoopState,
	NodeShape,
	NodeState,
	NodeStatus,
	Timestamp,
	Tone,
	WorkflowRun,
} from "@maxhq/shared/bindings";
import { formatAgoShort, formatClock } from "@maxhq/shared/status/format";
import type { TimelineBar } from "@maxhq/shared/workflow/canvas";
import { type GlyphColor, glyphOf } from "@maxhq/shared/workflow/glyphs";
import { loopCount, rowOf, type TabRow } from "@maxhq/shared/workflow/layout";
import { type RunChip, runChip, runStatus } from "@maxhq/shared/workflow/select";

import { color, tint } from "../../theme";
import type { Choice } from "../../ui/choose";

/** The color each glyph color draws in, as the desktop's. */
export const GLYPH_TINT: Readonly<Record<GlyphColor, string>> = {
	faint: color.mutedForeground,
	quiet: color.mutedForeground,
	muted: color.mutedForeground,
	plain: color.secondaryForeground,
	accent: color.primary,
	warn: color.busy,
	good: color.ready,
	bad: color.error,
};

/** The text color each tone reads in, for a run's notice and its loops. */
export const TONE_TEXT: Readonly<Record<Tone, string>> = {
	info: color.primary,
	neutral: color.mutedForeground,
	good: color.readyText,
	warn: color.busyText,
	bad: color.errorText,
};

/** How a run chip looks: amber while it waits on you, quiet once done. */
export const chipLook = (chip: RunChip): { readonly border: string; readonly text: string } =>
	chip.needsYou
		? { border: tint(color.busy, 0.4), text: color.busyText }
		: {
				border: color.track,
				text: chip.done ? color.mutedForeground : color.secondaryForeground,
			};

/** One run's row on the Workflows screen. */
export type RunLine = {
	readonly status: NodeStatus;
	readonly label: string;
	readonly workflow: string;
	/** Its progress while live, how long ago it ended once done. */
	readonly side: string;
	readonly needsYou: boolean;
	/** What it says to a screen reader. */
	readonly spoken: string;
};

export const runLine = (run: WorkflowRun, now: Timestamp): RunLine => ({
	status: runStatus(run),
	label: run.label,
	workflow: run.key.workflow,
	side: run.done
		? run.last_activity === null
			? ""
			: formatAgoShort(run.last_activity, now)
		: `${formatNumber(run.progress.done)}/${formatNumber(run.progress.total)}`,
	needsYou: run.needs_you && !run.done,
	spoken: runChip(run).spoken,
});

/**
 * A project's Workflows entry in the agent list: how many runs it has,
 * and the state of its liveliest live run, if one is live.
 */
export const workflowsEntry = (
	runs: readonly WorkflowRun[]
): { readonly count: number; readonly status: NodeStatus | null } => {
	const liveliest = runs.find((run) => run.needs_you && !run.done) ?? runs[0];
	return {
		count: runs.length,
		status: liveliest && !liveliest.done ? runStatus(liveliest) : null,
	};
};

/**
 * A run's menu: Open where the screen isn't the run's own, Stop run while
 * an agent works on it, and Copy run ID.
 */
export const runMenu = (actions: {
	readonly open?: () => void;
	readonly stop: (() => void) | null;
	readonly copy: () => void;
}): Choice[] => [
	...(actions.open ? [{ label: t("Open"), pick: actions.open }] : []),
	...(actions.stop ? [{ label: t("Stop run…"), pick: actions.stop, destructive: true }] : []),
	{ label: t("Copy run ID"), pick: actions.copy },
];

/** What a loop says to a screen reader, such as "review went round 2/3". */
export const loopLabel = (loop: LoopState): string =>
	t("{{value1}} went round {{value2}}", {
		value1: loop.label ?? t("Loop"),
		value2: loopCount(loop),
	});

/**
 * The loops that ran, by the row they went back from, which is a folded
 * group's row for a member. The phone shows each as a count on its row
 * instead of an arc.
 */
export const loopsByRow = (
	rows: readonly TabRow[],
	loops: readonly LoopState[]
): ReadonlyMap<number, readonly LoopState[]> => {
	const byRow = new Map<number, LoopState[]>();
	for (const loop of loops) {
		if (loop.count === 0) continue;
		const row = rowOf(rows, loop.from);
		if (row < 0) continue;
		byRow.set(row, [...(byRow.get(row) ?? []), loop]);
	}
	return byRow;
};

/**
 * What a node's row says to a screen reader: its name, how it stands, its
 * loops, and whether a tap opens its agent.
 */
export const nodeRowLabel = (
	shape: NodeShape,
	node: NodeState,
	agent: AgentId | null,
	loops: readonly LoopState[] = []
): string =>
	[shape.label, glyphOf(node.status, node.provisional).label, ...loops.map(loopLabel)].join(
		", "
	) + (agent ? t(", opens its agent") : "");

/**
 * What a node's sheet shows: how it stands and the workflow's note on it,
 * and the agents that worked on it that the phone has, latest first, to
 * open one.
 */
export const nodeSheet = (
	node: NodeState,
	titleOf: (agent: AgentId) => string | undefined
): {
	readonly message: string;
	readonly agents: readonly { readonly id: AgentId; readonly title: string }[];
} => {
	const ids = [
		...node.attempts.flatMap((attempt) => (attempt.agent === null ? [] : [attempt.agent])),
		...node.agents,
	].reverse();
	const agents = [...new Set(ids)].flatMap((id) => {
		const title = titleOf(id);
		return title === undefined ? [] : [{ id, title }];
	});
	const lines = [
		glyphOf(node.status, node.provisional).label,
		...node.badges.map((badge) => badge.text),
		...(node.note ? [node.note] : []),
	];
	return { message: lines.join("\n"), agents };
};

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

// The tick steps the axis picks from. A minute is the least, since a tick
// reads as a clock time.
const STEPS = [
	MINUTE,
	2 * MINUTE,
	5 * MINUTE,
	10 * MINUTE,
	15 * MINUTE,
	30 * MINUTE,
	HOUR,
	2 * HOUR,
	3 * HOUR,
	6 * HOUR,
	12 * HOUR,
	DAY,
	2 * DAY,
	7 * DAY,
];

/** The timeline's time axis, from a run's first attempt to its last. */
export type TimeAxis = {
	/** Where a time sits, from 0 to the chart's width. */
	readonly x: (at: Timestamp) => number;
	/** The times the axis marks, each a whole step, in order. */
	readonly ticks: readonly Timestamp[];
	/** What a tick reads: the clock time, or the date for steps of a day or more. */
	readonly label: (at: Timestamp) => string;
};

/**
 * A time axis `width` wide over `start` to `end`, at least a minute long,
 * widened to whole steps of local time, with a tick every 90 points or so,
 * as the desktop's d3 axis. `offset` is how far local time is ahead of
 * UTC, in milliseconds.
 */
export const timeAxis = (
	start: Timestamp,
	end: Timestamp,
	width: number,
	offset = -new Date(start).getTimezoneOffset() * MINUTE
): TimeAxis => {
	const to = Math.max(end, start + MINUTE);
	const count = Math.max(2, Math.floor(width / 90));
	const step = STEPS.find((each) => each >= (to - start) / count) ?? STEPS.at(-1)!;
	const low = Math.floor((start + offset) / step) * step - offset;
	const high = Math.ceil((to + offset) / step) * step - offset;
	const ticks: Timestamp[] = [];
	for (let at = low; at <= high; at += step) ticks.push(at);
	return {
		x: (at) => ((at - low) / (high - low)) * width,
		ticks,
		label:
			step >= DAY ? (at) => formatDate(at, { month: "short", day: "numeric" }) : formatClock,
	};
};

/**
 * Where a bar sits on the axis: its left edge, its width, at least 3
 * points, and how much of it is solid, which is all of it unless it waits
 * on you, as the desktop's.
 */
export const barGeometry = (
	bar: TimelineBar,
	x: (at: Timestamp) => number
): { readonly left: number; readonly width: number; readonly solid: number } => {
	const left = x(bar.start);
	const width = Math.max(3, x(bar.end) - left);
	const solid =
		bar.waitFrom === null ? width : Math.min(width, Math.max(0, x(bar.waitFrom) - left));
	return { left, width, solid };
};
