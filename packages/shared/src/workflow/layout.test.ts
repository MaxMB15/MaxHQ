// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { RunSnapshot } from "../bindings";
import { mockRunSnapshot, nodeState } from "../data/mock-workflow";
import { NOW } from "../test/build";
import {
	groupStatus,
	holdsAgent,
	loopArcs,
	loopCount,
	loopTone,
	nodeAgent,
	openAtFirst,
	rowOf,
	tabRows,
} from "./layout";

// Ui work with the mockup aside, one without it, and one after loops.
const SETTINGS = mockRunSnapshot(NOW, "t-20261101-example-settings");
const FILTERS = mockRunSnapshot(NOW, "t-20261101-example-filters");
const SYNC = mockRunSnapshot(NOW, "t-20261101-example-sync");

const rowsOf = (run: RunSnapshot, open: ReadonlySet<string> = openAtFirst(run.shape)) => {
	const status = new Map(run.nodes.map((node) => [node.id, node.status]));
	return tabRows(run.shape, (id) => status.get(id), open);
};

const ids = (run: RunSnapshot, open?: ReadonlySet<string>) =>
	rowsOf(run, open).map((row) =>
		row.kind === "node" ? `${row.node.id}@${row.lane}${row.grouped ? "g" : ""}` : `[${row.id}]`
	);

const TAIL = ["verify@0", "integrate@0", "review@0", "ci@0"];

describe("tabRows", () => {
	it("folds a group into its first member's place and puts laned nodes aside", () => {
		expect(ids(SETTINGS)).toEqual([
			"research@0",
			"design@0",
			"mockup@1",
			"approve@0",
			"implement@0",
			"[analysis]",
			...TAIL,
		]);
	});

	it("leaves omitted nodes out", () => {
		expect(ids(FILTERS)).not.toContain("mockup@1");
		expect(ids(FILTERS)).toHaveLength(9);
	});

	it("lists an open group's members after its row, bracketed", () => {
		expect(ids(FILTERS, new Set(["analysis"]))).toEqual([
			"research@0",
			"design@0",
			"approve@0",
			"implement@0",
			"[analysis]",
			"testing@0g",
			"security@0g",
			"a11y@0g",
			"chaos@0g",
			"perf@0g",
			...TAIL,
		]);
	});

	it("opens at first the groups the workflow doesn't fold", () => {
		expect(openAtFirst(SETTINGS.shape)).toEqual(new Set());
		const groups = SETTINGS.shape.groups.map((group) => ({ ...group, collapsed: false }));
		expect(openAtFirst({ ...SETTINGS.shape, groups })).toEqual(new Set(["analysis"]));
	});

	it("places a node on its folded group's row", () => {
		expect(rowOf(rowsOf(SETTINGS), "security")).toBe(5);
		expect(rowOf(rowsOf(FILTERS), "mockup")).toBe(-1);
	});
});

describe("groupStatus", () => {
	it("shows the most pressing member, and done once all are settled", () => {
		expect(groupStatus(["done", "busy", "failed"])).toBe("failed");
		expect(groupStatus(["done", "waiting_human", "failed"])).toBe("waiting_human");
		expect(groupStatus(["done", "skipped"])).toBe("done");
		expect(groupStatus(["skipped"])).toBe("skipped");
	});
});

describe("loops", () => {
	const loop = { from: "approve", to: "mockup", label: null, count: 1, budget: 3 };

	it("turns amber on the last allowed loop and red once spent", () => {
		expect(loopTone(loop)).toBe("info");
		expect(loopTone({ ...loop, count: 2 })).toBe("warn");
		expect(loopTone({ ...loop, count: 3 })).toBe("bad");
		expect(loopTone({ ...loop, budget: null, count: 9 })).toBe("info");
	});

	it("counts against the budget when there is one", () => {
		expect(loopCount(loop)).toBe("1/3");
		expect(loopCount({ ...loop, budget: null })).toBe("1");
	});

	it("draws each loop that ran from the row it went back from up to the one it went to", () => {
		expect(loopArcs(rowsOf(SETTINGS), SETTINGS.loops)).toEqual([
			{ loop: SETTINGS.loops[0], from: 3, to: 2, tone: "info" },
		]);
		expect(
			loopArcs(rowsOf(SYNC), SYNC.loops).map(({ from, to, tone }) => [from, to, tone])
		).toEqual([
			[6, 3, "warn"],
			[5, 3, "info"],
			[8, 3, "info"],
		]);
	});

	it("leaves out a loop that hasn't run, or one inside one folded row", () => {
		const rows = rowsOf(SETTINGS);
		expect(loopArcs(rows, [{ ...loop, count: 0 }])).toEqual([]);
		expect(loopArcs(rows, [{ ...loop, from: "testing", to: "security" }])).toEqual([]);
	});
});

describe("nodeAgent", () => {
	const attempt = { start: 1, end: null, end_status: null, agent: null };
	const node = (id: string) => SETTINGS.nodes.find((each) => each.id === id)!;

	it("opens the latest attempt with an agent, then the last bound one", () => {
		expect(nodeAgent(node("mockup"), "agent-lead")).toBe("agent-ex-mockup");
		const build = nodeState("build", {
			status: "busy",
			attempts: [
				{ ...attempt, agent: "agent-a" },
				{ ...attempt, agent: null },
			],
			agents: ["agent-a", "agent-b"],
		});
		expect(nodeAgent(build, "agent-lead")).toBe("agent-a");
		expect(nodeAgent({ ...build, attempts: [] }, "agent-lead")).toBe("agent-b");
	});

	it("opens the orchestrator only while a node with no agent is under way", () => {
		expect(nodeAgent(node("approve"), "agent-lead")).toBe("agent-lead");
		expect(nodeAgent(node("research"), "agent-lead")).toBeNull();
		expect(nodeAgent(nodeState("gate", { status: "busy" }), null)).toBeNull();
	});
});

describe("holdsAgent", () => {
	it("matches a node bound to the agent or with an attempt by it", () => {
		const node = nodeState("build", {
			status: "done",
			agents: ["agent-a"],
			attempts: [{ start: 1, end: 2, end_status: "failed", agent: "agent-b" }],
		});
		expect(holdsAgent(node, "agent-a")).toBe(true);
		expect(holdsAgent(node, "agent-b")).toBe(true);
		expect(holdsAgent(node, "agent-c")).toBe(false);
		expect(holdsAgent(node, null)).toBe(false);
	});
});
