// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { WorkflowRun } from "../bindings";
import { agent, entities, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { stopRunQuestion, stopTargets } from "./stop";

const run = (over: Partial<WorkflowRun> = {}): WorkflowRun => ({
	id: "wfr-1",
	key: { device: "d1", project: "p1", workflow: "pipeline", run: "r1" },
	label: "Add sign-in",
	progress: { done: 1, total: 3 },
	current: [],
	needs_you: false,
	orchestrators: ["lead"],
	last_activity: NOW,
	done: false,
	...over,
});

describe("Stop run", () => {
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			agents: [agent({ id: "lead" }), agent({ id: "node" })],
		}),
	});

	it("stops the leads and node agents the view still has", () => {
		expect(stopTargets(view, run({ agents: ["node", "lead", "gone"] }))).toEqual([
			"lead",
			"node",
		]);
	});

	it("stops nothing once the run finished", () => {
		expect(stopTargets(view, run({ done: true, agents: ["node"] }))).toEqual([]);
	});

	it("names the run and what stopping loses", () => {
		const question = stopRunQuestion(run(), 2);
		expect(question.title).toBe('Stop "Add sign-in"?');
		expect(question.detail).toContain("is lost");
	});
});
