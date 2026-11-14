// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { NodeState, RunKey, RunSnapshot, WorkflowUpdate } from "../bindings";
import type { DataSource } from "../data/source";
import { applyRunUpdate, EMPTY_RUN, openRun } from "./store";

const RUN: RunKey = { device: "d1", project: "p1", workflow: "pipeline", run: "t-1" };

const node = (id: string, status: NodeState["status"] = "queued"): NodeState => ({
	id,
	status,
	provisional: false,
	note: null,
	badges: [],
	attempts: [],
	agents: [],
	quiet: false,
	links: [],
});

const snapshot = (rev: number, hash = "h1"): RunSnapshot => ({
	key: RUN,
	rev,
	shape: {
		nodes: [
			{ id: "a", label: "A", group: null, lane: null, host: null },
			{ id: "b", label: "B", group: null, lane: null, host: null },
		],
		edges: [{ from: "a", to: "b" }],
		groups: [],
		hash,
	},
	nodes: [node("a"), node("b")],
	loops: [],
	header: { label: "Run", status: "busy", progress: { done: 0, total: 2 } },
	notice: null,
});

const reset = (rev: number, hash?: string): WorkflowUpdate => ({
	kind: "reset",
	run: snapshot(rev, hash),
});

const patch = (rev: number, ...nodes: NodeState[]): WorkflowUpdate => ({
	kind: "patch",
	run: { key: RUN, rev, nodes, loops: null, header: null },
});

const ready = (update: WorkflowUpdate) => applyRunUpdate(EMPTY_RUN, update)!;

describe("applyRunUpdate", () => {
	it("fills the run from a reset", () => {
		const state = ready(reset(4));
		expect(state.status).toBe("ready");
		expect(state.rev).toBe(4);
		expect([...state.nodes.keys()]).toEqual(["a", "b"]);
		expect(state.header?.label).toBe("Run");
	});

	it("applies patches in revision order and keeps unchanged nodes as they were", () => {
		const start = ready(reset(1));
		const next = applyRunUpdate(start, patch(2, node("a", "busy")))!;
		expect(next.rev).toBe(2);
		expect(next.nodes.get("a")?.status).toBe("busy");
		expect(next.nodes.get("b")).toBe(start.nodes.get("b"));
		expect(next.shape).toBe(start.shape);
		const after = applyRunUpdate(next, patch(3, node("b", "done")))!;
		expect(after.nodes.get("a")).toBe(next.nodes.get("a"));
		expect(after.nodes.get("b")?.status).toBe("done");
	});

	it("turns down a patch that skips a revision or comes before a reset", () => {
		expect(applyRunUpdate(ready(reset(1)), patch(3, node("a", "busy")))).toBeNull();
		expect(applyRunUpdate(ready(reset(5)), patch(5))).toBeNull();
		expect(applyRunUpdate(EMPTY_RUN, patch(1))).toBeNull();
	});

	it("keeps each unchanged node's object through a reset", () => {
		const start = applyRunUpdate(ready(reset(1)), patch(2, node("a", "busy")))!;
		const again = applyRunUpdate(start, {
			kind: "reset",
			run: { ...snapshot(9), nodes: [node("a", "busy"), node("b", "done")] },
		})!;
		expect(again.nodes.get("a")).toBe(start.nodes.get("a"));
		expect(again.nodes.get("b")).not.toBe(start.nodes.get("b"));
		expect(again.nodes.get("b")?.status).toBe("done");
	});

	it("keeps the shape object when a reset brings the same shape", () => {
		const start = ready(reset(1));
		expect(applyRunUpdate(start, reset(7))!.shape).toBe(start.shape);
		expect(applyRunUpdate(start, reset(7, "h2"))!.shape).not.toBe(start.shape);
	});
});

/** A source whose workflow opens a test feeds by hand. */
const fakeSource = () => {
	const opens: Array<{ push: (update: WorkflowUpdate) => void; stop: () => void }> = [];
	const source = {
		openWorkflow: vi.fn(async (_run: RunKey, onUpdate: (update: WorkflowUpdate) => void) => {
			const stop = vi.fn();
			opens.push({ push: onUpdate, stop });
			return stop;
		}),
	} as unknown as DataSource;
	return { source, opens };
};

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("openRun", () => {
	it("opens again on a gap and drops the open it replaced once the new one is in", async () => {
		const { source, opens } = fakeSource();
		const run = openRun(source, RUN);
		await settle();
		opens[0]!.push(reset(1));
		opens[0]!.push(patch(2, node("a", "busy")));
		expect(run.store.getState().rev).toBe(2);

		opens[0]!.push(patch(4, node("b", "done")));
		expect(opens).toHaveLength(2);
		expect(run.store.getState().rev).toBe(2);
		await settle();
		expect(opens[0]!.stop).toHaveBeenCalled();

		// The old open is gone, so what it still sends counts for nothing.
		opens[0]!.push(patch(3, node("b", "failed")));
		expect(run.store.getState().nodes.get("b")?.status).toBe("queued");
		opens[1]!.push(reset(4));
		expect(run.store.getState().rev).toBe(4);
		run.close();
		expect(opens[1]!.stop).toHaveBeenCalled();
	});

	it("says why a run couldn't open", async () => {
		const source = {
			openWorkflow: async () => {
				throw new Error("No such run");
			},
		} as unknown as DataSource;
		const run = openRun(source, RUN);
		await settle();
		expect(run.store.getState()).toMatchObject({ status: "failed", error: "No such run" });
	});

	it("closes an open that lands after the run closed", async () => {
		const { source, opens } = fakeSource();
		const run = openRun(source, RUN);
		run.close();
		await settle();
		expect(opens[0]!.stop).toHaveBeenCalled();
	});
});
