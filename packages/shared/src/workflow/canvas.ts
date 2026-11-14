// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the canvas draws, worked out from a run: where each card and group
// box sits, the edges between cards, the loops on a lane to their right,
// and the timeline's rows and bars. The layout comes from ELK, which a
// view runs on the graph `elkGraph` builds and reads back with `fromElk`,
// or from `columnLayout` where ELK can't run. It depends only on the
// shape, so a view lays a run out once per `shape.hash`.

import { t } from "../i18n";

import type {
	AgentId,
	LoopState,
	NodeShape,
	NodeState,
	NodeStatus,
	RunShape,
	Tone,
} from "../bindings";
import { loopTone } from "./layout";

/** A card's size. */
export const CARD = { width: 200, height: 56 } as const;

// The room around a group's members, with its label on top.
const PAD = { top: 30, side: 12, bottom: 12 } as const;
// The gap between cards in the fallback column.
const GAP = 28;
// How far right of the widest node the first loop runs, and between loops.
const LANE = { first: 28, step: 16 } as const;

export type Point = { readonly x: number; readonly y: number };

export type Box = {
	readonly x: number;
	readonly y: number;
	readonly width: number;
	readonly height: number;
};

/** A group's box around its members. */
export type GroupBox = Box & {
	readonly id: string;
	readonly label: string;
	/** Its members' ids, in the workflow's order. */
	readonly members: readonly string[];
};

/** An edge's route, from the bottom of the node needed to the top of the one that needs it. */
export type CanvasEdge = {
	readonly id: string;
	readonly from: string;
	readonly to: string;
	readonly points: readonly Point[];
};

/** Where everything sits, in one coordinate space. */
export type CanvasLayout = {
	/** The shape's hash, which this layout is for. */
	readonly hash: string;
	/** Each card's box, by node id. */
	readonly nodes: ReadonlyMap<string, Box>;
	readonly groups: readonly GroupBox[];
	readonly edges: readonly CanvasEdge[];
};

/** The part of an ELK graph the canvas builds and reads. */
export type ElkGraph = {
	readonly id: string;
	readonly layoutOptions?: Readonly<Record<string, string>>;
	readonly x?: number;
	readonly y?: number;
	readonly width?: number;
	readonly height?: number;
	readonly children?: readonly ElkGraph[];
	readonly edges?: readonly {
		readonly id: string;
		readonly sources: readonly string[];
		readonly targets: readonly string[];
		readonly sections?: readonly {
			readonly startPoint: Point;
			readonly endPoint: Point;
			readonly bendPoints?: readonly Point[];
		}[];
	}[];
};

/** The options the canvas lays out with. Coordinates come back from the root. */
export const ELK_OPTIONS: Readonly<Record<string, string>> = {
	"elk.algorithm": "layered",
	"elk.direction": "DOWN",
	"elk.hierarchyHandling": "INCLUDE_CHILDREN",
	"elk.layered.considerModelOrder.strategy": "NODES_AND_EDGES",
	"elk.edgeRouting": "ORTHOGONAL",
	"elk.layered.nodePlacement.strategy": "BRANDES_KOEPF",
	"elk.spacing.nodeNode": "28",
	"elk.layered.spacing.nodeNodeBetweenLayers": "40",
	"elk.json.shapeCoords": "ROOT",
	"elk.json.edgeCoords": "ROOT",
};

// Nodes and groups each have ids unique among their own kind, so ELK gets
// them under a prefix each.
const nodeKey = (id: string) => `n:${id}`;
const groupKey = (id: string) => `g:${id}`;

/**
 * The edges ELK lays out: the ones that point forward in the workflow's
 * order between nodes it has, once each. A loop goes back, so it has its
 * own lane instead.
 */
export const forwardEdges = (shape: RunShape): { from: string; to: string }[] => {
	const order = new Map(shape.nodes.map((node, i) => [node.id, i]));
	const seen = new Set<string>();
	return shape.edges.filter(({ from, to }) => {
		const a = order.get(from);
		const b = order.get(to);
		const key = `${from}\n${to}`;
		if (a === undefined || b === undefined || a >= b || seen.has(key)) return false;
		seen.add(key);
		return true;
	});
};

// The shape in the workflow's order, with each group in the place of its
// first member and its members inside it.
type Item =
	| { readonly kind: "node"; readonly node: NodeShape }
	| { readonly kind: "group"; readonly id: string; readonly label: string; members: NodeShape[] };

const itemsOf = (shape: RunShape): Item[] => {
	const groups = new Map(shape.groups.map((group) => [group.id, group]));
	const items: Item[] = [];
	const placed = new Map<string, Extract<Item, { kind: "group" }>>();
	for (const node of shape.nodes) {
		const group = node.group === null ? undefined : groups.get(node.group);
		if (!group) {
			items.push({ kind: "node", node });
			continue;
		}
		let item = placed.get(group.id);
		if (!item) {
			item = { kind: "group", id: group.id, label: group.label, members: [] };
			placed.set(group.id, item);
			items.push(item);
		}
		item.members.push(node);
	}
	return items;
};

const card = (node: NodeShape): ElkGraph => ({ id: nodeKey(node.id), ...CARD });

/** The graph ELK lays a shape out from. */
export const elkGraph = (shape: RunShape): ElkGraph => ({
	id: "root",
	layoutOptions: ELK_OPTIONS,
	children: itemsOf(shape).map((item) =>
		item.kind === "node"
			? card(item.node)
			: {
					id: groupKey(item.id),
					layoutOptions: {
						"elk.direction": "DOWN",
						"elk.padding": `[top=${PAD.top},left=${PAD.side},bottom=${PAD.bottom},right=${PAD.side}]`,
					},
					children: item.members.map(card),
				}
	),
	edges: forwardEdges(shape).map(({ from, to }, i) => ({
		id: `e${i}`,
		sources: [nodeKey(from)],
		targets: [nodeKey(to)],
	})),
});

const boxOf = (graph: ElkGraph): Box => ({
	x: graph.x ?? 0,
	y: graph.y ?? 0,
	width: graph.width ?? CARD.width,
	height: graph.height ?? CARD.height,
});

// A straight route between two cards, for an edge ELK gave no route.
const straight = (from: Box, to: Box): Point[] => [
	{ x: from.x + from.width / 2, y: from.y + from.height },
	{ x: to.x + to.width / 2, y: to.y },
];

/** A layout from what ELK returned for `elkGraph(shape)`. */
export const fromElk = (shape: RunShape, laid: ElkGraph): CanvasLayout => {
	const nodes = new Map<string, Box>();
	const groups: GroupBox[] = [];
	const labels = new Map(shape.groups.map((group) => [group.id, group.label]));
	for (const child of laid.children ?? []) {
		if (child.id.startsWith("n:")) {
			nodes.set(child.id.slice(2), boxOf(child));
			continue;
		}
		const id = child.id.slice(2);
		const members = (child.children ?? []).map((member) => member.id.slice(2));
		for (const member of child.children ?? []) nodes.set(member.id.slice(2), boxOf(member));
		groups.push({ ...boxOf(child), id, label: labels.get(id) ?? id, members });
	}
	const edges = forwardEdges(shape).flatMap(({ from, to }, i): CanvasEdge[] => {
		const a = nodes.get(from);
		const b = nodes.get(to);
		if (!a || !b) return [];
		const section = laid.edges?.find((edge) => edge.id === `e${i}`)?.sections?.[0];
		const points = section
			? [section.startPoint, ...(section.bendPoints ?? []), section.endPoint]
			: straight(a, b);
		return [{ id: `${from}->${to}`, from, to, points }];
	});
	return { hash: shape.hash, nodes, groups, edges };
};

/**
 * A layout with no ELK: one column, top down, in the workflow's order,
 * with each group's members boxed together. Tests use it, and so does a
 * canvas whose layout worker failed.
 */
export const columnLayout = (shape: RunShape): CanvasLayout => {
	const nodes = new Map<string, Box>();
	const groups: GroupBox[] = [];
	const x = PAD.side;
	let y = 0;
	const place = (node: NodeShape) => {
		nodes.set(node.id, { x, y, ...CARD });
		y += CARD.height + GAP;
	};
	for (const item of itemsOf(shape)) {
		if (item.kind === "node") {
			place(item.node);
			continue;
		}
		const top = y;
		y += PAD.top;
		for (const member of item.members) place(member);
		y += PAD.bottom - GAP;
		groups.push({
			id: item.id,
			label: item.label,
			members: item.members.map((member) => member.id),
			x: 0,
			y: top,
			width: CARD.width + 2 * PAD.side,
			height: y - top,
		});
		y += GAP;
	}
	const edges = forwardEdges(shape).map(({ from, to }) => ({
		id: `${from}->${to}`,
		from,
		to,
		points: straight(nodes.get(from)!, nodes.get(to)!),
	}));
	return { hash: shape.hash, nodes, groups, edges };
};

/** A loop's route on the lane right of the widest node. */
export type LoopRoute = {
	readonly loop: LoopState;
	readonly points: readonly Point[];
	/** Where its counter sits. */
	readonly label: Point;
	/** Whether it hasn't run yet, which draws it dashed and quiet. */
	readonly possible: boolean;
	readonly tone: Tone;
};

// What a loop's end is drawn at: a card, or the box of a group it goes back to.
const endOf = (layout: CanvasLayout, id: string): Box | undefined =>
	layout.nodes.get(id) ?? layout.groups.find((group) => group.id === id);

/**
 * The loops to draw, each on a lane of its own right of the widest node,
 * the shortest nearest. Loops that haven't run show only with `possible`.
 */
export const loopRoutes = (
	layout: CanvasLayout,
	loops: readonly LoopState[],
	possible: boolean
): LoopRoute[] => {
	const boxes = [...layout.nodes.values(), ...layout.groups];
	const right = Math.max(0, ...boxes.map((box) => box.x + box.width));
	const placed = loops.flatMap((loop) => {
		if (loop.count === 0 && !possible) return [];
		const from = endOf(layout, loop.from);
		const to = endOf(layout, loop.to);
		if (!from || !to) return [];
		const middle = (box: Box) => box.y + box.height / 2;
		// A loop on one node leaves its lower half and comes back to its upper.
		const self = from === to;
		const y0 = self ? middle(from) + from.height / 4 : middle(from);
		const y1 = self ? middle(to) - to.height / 4 : middle(to);
		return [{ loop, from, to, y0, y1 }];
	});
	placed.sort((a, b) => Math.abs(a.y0 - a.y1) - Math.abs(b.y0 - b.y1));
	return placed.map(({ loop, from, to, y0, y1 }, i) => {
		const lane = right + LANE.first + i * LANE.step;
		return {
			loop,
			points: [
				{ x: from.x + from.width, y: y0 },
				{ x: lane, y: y0 },
				{ x: lane, y: y1 },
				{ x: to.x + to.width, y: y1 },
			],
			label: { x: lane, y: (y0 + y1) / 2 },
			possible: loop.count === 0,
			tone: loop.count === 0 ? "neutral" : loopTone(loop),
		};
	});
};

/** An SVG path through points, in straight lines. */
export const pathThrough = (points: readonly Point[]): string =>
	points.map((p, i) => `${i === 0 ? "M" : "L"} ${p.x} ${p.y}`).join(" ");

/** One attempt on the timeline. */
export type TimelineBar = {
	readonly start: number;
	/** When it ended, or now while it runs. */
	readonly end: number;
	readonly running: boolean;
	/** How it ended, or where its node stands while it runs. */
	readonly status: NodeStatus;
	/** When it began waiting on you, hatched from there to its end, or null. */
	readonly waitFrom: number | null;
	readonly agent: AgentId | null;
};

/** One node's row on the timeline. */
export type TimelineRow = {
	readonly node: NodeShape;
	readonly bars: readonly TimelineBar[];
};

/** The timeline of a run. */
export type Timeline = {
	readonly rows: readonly TimelineRow[];
	/** The earliest start, or now when nothing started. */
	readonly start: number;
	/** The latest end, which is now while anything runs. */
	readonly end: number;
	/** What the run doesn't say that a hatch needs, for a person to read. */
	readonly missing: readonly string[];
};

/**
 * One row per node the run doesn't leave out, one bar per attempt. A node
 * that waits on you hatches its open attempt from when the wait began. A
 * run that doesn't say when hatches the whole attempt and says so.
 */
export const timeline = (
	shape: RunShape,
	nodes: ReadonlyMap<string, NodeState>,
	now: number
): Timeline => {
	const missing: string[] = [];
	let start = Infinity;
	let end = -Infinity;
	const rows = shape.nodes.flatMap((node): TimelineRow[] => {
		const state = nodes.get(node.id);
		if (!state || state.status === "omitted") return [];
		const waits = state.status === "waiting_human";
		const bars = state.attempts.map((attempt): TimelineBar => {
			const running = attempt.end === null;
			const barEnd = attempt.end ?? Math.max(now, attempt.start);
			const since = attempt.waiting_since ?? attempt.start;
			const bar = {
				start: attempt.start,
				end: barEnd,
				running,
				status: running ? state.status : (attempt.end_status ?? "done"),
				waitFrom:
					running && waits ? Math.min(Math.max(since, attempt.start), barEnd) : null,
				agent: attempt.agent,
			};
			start = Math.min(start, bar.start);
			end = Math.max(end, bar.end);
			return bar;
		});
		const open = state.attempts.find((attempt) => attempt.end === null);
		if (waits && !open) {
			missing.push(
				t("{{node_label}} waits on you, but the run has no times for it", {
					node_label: node.label,
				})
			);
		} else if (waits && open && (open.waiting_since ?? null) === null) {
			missing.push(
				t(
					"{{node_label}} waits on you, but the run doesn't say since when, so its whole open attempt is hatched",
					{ node_label: node.label }
				)
			);
		}
		return [{ node, bars }];
	});
	return start === Infinity
		? { rows, start: now, end: now, missing }
		: { rows, start, end, missing };
};
