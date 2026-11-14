// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { LoopState, RunShape, RunSnapshot } from "../bindings";
import { mockRunSnapshot, nodeState } from "../data/mock-workflow";
import { NOW } from "../test/build";
import {
	CARD,
	columnLayout,
	ELK_OPTIONS,
	type ElkGraph,
	elkGraph,
	forwardEdges,
	fromElk,
	loopRoutes,
	timeline,
} from "./canvas";

const SETTINGS = mockRunSnapshot(NOW, "t-20261101-example-settings");
const SYNC = mockRunSnapshot(NOW, "t-20261101-example-sync");

const nodesOf = (run: RunSnapshot) => new Map(run.nodes.map((node) => [node.id, node]));

const SMALL: RunShape = {
	nodes: [
		{ id: "a", label: "A", group: null, lane: null, host: null },
		{ id: "b", label: "B", group: "g", lane: null, host: null },
		{ id: "c", label: "C", group: "g", lane: null, host: null },
		{ id: "d", label: "D", group: null, lane: null, host: null },
	],
	edges: [
		{ from: "a", to: "b" },
		{ from: "a", to: "c" },
		{ from: "b", to: "d" },
		{ from: "c", to: "d" },
		{ from: "d", to: "a" },
		{ from: "a", to: "b" },
		{ from: "a", to: "gone" },
	],
	groups: [{ id: "g", label: "Checks", collapsed: false }],
	hash: "small",
};

const loop = (from: string, to: string, count: number, budget: number | null = 3): LoopState => ({
	from,
	to,
	label: null,
	count,
	budget,
});

describe("forwardEdges", () => {
	it("keeps each edge that points forward once, and drops loops and unknown ends", () => {
		expect(forwardEdges(SMALL)).toEqual([
			{ from: "a", to: "b" },
			{ from: "a", to: "c" },
			{ from: "b", to: "d" },
			{ from: "c", to: "d" },
		]);
	});
});

describe("elkGraph", () => {
	it("lays out with the canvas options, groups as boxes around their members", () => {
		const graph = elkGraph(SMALL);
		expect(graph.layoutOptions).toBe(ELK_OPTIONS);
		expect(ELK_OPTIONS).toMatchObject({
			"elk.algorithm": "layered",
			"elk.direction": "DOWN",
			"elk.hierarchyHandling": "INCLUDE_CHILDREN",
			"elk.layered.considerModelOrder.strategy": "NODES_AND_EDGES",
			"elk.edgeRouting": "ORTHOGONAL",
			"elk.layered.nodePlacement.strategy": "BRANDES_KOEPF",
		});
		expect(graph.children?.map((child) => child.id)).toEqual(["n:a", "g:g", "n:d"]);
		expect(graph.children?.[1]?.children).toEqual([
			{ id: "n:b", ...CARD },
			{ id: "n:c", ...CARD },
		]);
		expect(graph.edges?.map((edge) => [edge.sources[0], edge.targets[0]])).toEqual([
			["n:a", "n:b"],
			["n:a", "n:c"],
			["n:b", "n:d"],
			["n:c", "n:d"],
		]);
	});
});

describe("fromElk", () => {
	it("reads boxes and routes back by node id", () => {
		const laid: ElkGraph = {
			id: "root",
			children: [
				{ id: "n:a", x: 100, y: 0, ...CARD },
				{
					id: "g:g",
					x: 0,
					y: 100,
					width: 450,
					height: 100,
					children: [
						{ id: "n:b", x: 12, y: 130, ...CARD },
						{ id: "n:c", x: 238, y: 130, ...CARD },
					],
				},
				{ id: "n:d", x: 100, y: 260, ...CARD },
			],
			edges: [
				{
					id: "e0",
					sources: ["n:a"],
					targets: ["n:b"],
					sections: [
						{
							startPoint: { x: 200, y: 56 },
							bendPoints: [
								{ x: 200, y: 80 },
								{ x: 112, y: 80 },
							],
							endPoint: { x: 112, y: 130 },
						},
					],
				},
			],
		};
		const layout = fromElk(SMALL, laid);
		expect(layout.hash).toBe("small");
		expect(layout.nodes.get("c")).toEqual({ x: 238, y: 130, ...CARD });
		expect(layout.groups).toEqual([
			{
				id: "g",
				label: "Checks",
				members: ["b", "c"],
				x: 0,
				y: 100,
				width: 450,
				height: 100,
			},
		]);
		expect(layout.edges[0]?.points).toHaveLength(4);
		// An edge ELK left unrouted goes straight down.
		expect(layout.edges[1]?.points).toEqual([
			{ x: 200, y: 56 },
			{ x: 338, y: 130 },
		]);
	});
});

describe("columnLayout", () => {
	it("stacks every node top down, in order, with groups boxed", () => {
		const layout = columnLayout(SMALL);
		const ys = SMALL.nodes.map((node) => layout.nodes.get(node.id)!.y);
		expect(ys).toEqual([...ys].sort((a, b) => a - b));
		const [group] = layout.groups;
		expect(group?.members).toEqual(["b", "c"]);
		const b = layout.nodes.get("b")!;
		const c = layout.nodes.get("c")!;
		expect(b.y).toBeGreaterThan(group!.y);
		expect(c.y + c.height).toBeLessThan(group!.y + group!.height);
		expect(layout.nodes.get("d")!.y).toBeGreaterThan(group!.y + group!.height);
		expect(layout.edges).toHaveLength(4);
	});

	it("lays out the omitted nodes too, so it depends on the shape alone", () => {
		expect(columnLayout(SETTINGS.shape).nodes.size).toBe(SETTINGS.shape.nodes.length);
	});
});

describe("loopRoutes", () => {
	const layout = columnLayout(SMALL);
	const right = Math.max(...layout.groups.map((g) => g.x + g.width));

	it("draws loops that ran right of the widest node, the shortest nearest", () => {
		const routes = loopRoutes(
			layout,
			[loop("d", "a", 2), loop("c", "b", 1), loop("d", "b", 0)],
			false
		);
		expect(routes.map((r) => `${r.loop.from}-${r.loop.to}`)).toEqual(["c-b", "d-a"]);
		expect(routes[0]!.points[1]!.x).toBeGreaterThan(right);
		expect(routes[1]!.points[1]!.x).toBeGreaterThan(routes[0]!.points[1]!.x);
		expect(routes[1]!.tone).toBe("warn");
	});

	it("draws loops that haven't run, quiet, when asked", () => {
		const routes = loopRoutes(layout, [loop("d", "b", 0)], true);
		expect(routes).toHaveLength(1);
		expect(routes[0]).toMatchObject({ possible: true, tone: "neutral" });
	});

	it("goes back to a group's box, and around one node for a loop on itself", () => {
		const routes = loopRoutes(layout, [loop("d", "g", 1), loop("a", "a", 1)], false);
		const toGroup = routes.find((r) => r.loop.to === "g");
		const self = routes.find((r) => r.loop.to === "a");
		const group = layout.groups[0]!;
		expect(toGroup!.points.at(-1)).toEqual({
			x: group.x + group.width,
			y: group.y + group.height / 2,
		});
		const a = layout.nodes.get("a")!;
		expect(self!.points[0]!.y).toBeGreaterThan(self!.points.at(-1)!.y);
		expect(self!.points[0]!.x).toBe(a.x + a.width);
	});

	it("draws the mock run's loops that ran", () => {
		const routes = loopRoutes(columnLayout(SYNC.shape), SYNC.loops, false);
		expect(routes.length).toBe(SYNC.loops.filter((l) => l.count > 0).length);
	});
});

describe("timeline", () => {
	it("has a row per node the run doesn't leave out, a bar per attempt", () => {
		const line = timeline(SYNC.shape, nodesOf(SYNC), NOW);
		const omitted = SYNC.nodes.filter((n) => n.status === "omitted").length;
		expect(line.rows).toHaveLength(SYNC.shape.nodes.length - omitted);
		const implement = line.rows.find((row) => row.node.id === "implement")!;
		const state = SYNC.nodes.find((n) => n.id === "implement")!;
		expect(implement.bars).toHaveLength(state.attempts.length);
		expect(line.start).toBeLessThanOrEqual(line.end);
		expect(line.end).toBeLessThanOrEqual(NOW);
	});

	it("hatches a node that waits on you from when the wait began", () => {
		const line = timeline(SETTINGS.shape, nodesOf(SETTINGS), NOW);
		const approve = line.rows.find((row) => row.node.id === "approve")!;
		const open = SETTINGS.nodes.find((n) => n.id === "approve")!.attempts.at(-1)!;
		expect(open.waiting_since).toBeGreaterThan(open.start);
		expect(approve.bars.at(-1)).toMatchObject({
			running: true,
			waitFrom: open.waiting_since,
			end: NOW,
		});
		expect(line.missing).toEqual([]);
	});

	it("hatches the whole open attempt and says so when the run doesn't say since when", () => {
		const state = {
			...nodeState("a", { status: "waiting_human" }),
			attempts: [{ start: NOW - 50, end: null, end_status: null, agent: null }],
		};
		const line = timeline(SMALL, new Map([["a", state]]), NOW);
		expect(line.rows[0]?.bars[0]).toMatchObject({
			running: true,
			waitFrom: NOW - 50,
			end: NOW,
		});
		expect(line.missing).toHaveLength(1);
		expect(line.missing[0]).toMatch(/doesn't say since when, so its whole open attempt/);
	});

	it("says when a node waits on you with no attempt to hatch", () => {
		const waiting = nodeState("a", { status: "waiting_human" });
		const line = timeline(SMALL, new Map([["a", { ...waiting, attempts: [] }]]), NOW);
		expect(line.rows[0]?.bars).toEqual([]);
		expect(line.missing[0]).toMatch(/has no times for it/);
		expect(line).toMatchObject({ start: NOW, end: NOW });
	});

	it("ends a closed attempt where it ended, with how it ended", () => {
		const state = {
			...nodeState("a", { status: "done" }),
			attempts: [
				{ start: NOW - 50, end: NOW - 20, end_status: "failed" as const, agent: null },
			],
		};
		const line = timeline(SMALL, new Map([["a", state]]), NOW);
		expect(line.rows[0]?.bars[0]).toMatchObject({
			start: NOW - 50,
			end: NOW - 20,
			status: "failed",
			waitFrom: null,
		});
		expect(line).toMatchObject({ start: NOW - 50, end: NOW - 20 });
	});
});
