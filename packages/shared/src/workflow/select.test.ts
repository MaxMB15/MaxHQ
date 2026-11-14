// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, RunKey, WorkflowRun } from "../bindings";
import { MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	chipOf,
	leadOf,
	liveRunAgents,
	projectRuns,
	runAt,
	runName,
	runsOf,
	runStatus,
	sameRun,
	unboundRuns,
} from "./select";

const key = (run: string, project = "p1"): RunKey => ({
	device: "d1",
	project,
	workflow: "pipeline",
	run,
});

const run = (id: string, over: Partial<WorkflowRun> = {}): Entity => ({
	entity: "workflow_run",
	id: `wfr-${id}`,
	key: key(id),
	label: id,
	progress: { done: 3, total: 9 },
	current: [],
	needs_you: false,
	orchestrators: [],
	last_activity: NOW,
	done: false,
	...over,
});

const viewOf = (...entities: Entity[]) => applySnapshot({ seq: 1, entities });

describe("workflow selectors", () => {
	it("names a run by every part of its key", () => {
		expect(runName(key("t-1"))).toBe("d1/p1/pipeline/t-1");
		expect(sameRun(key("t-1"), key("t-1"))).toBe(true);
		expect(sameRun(key("t-1"), key("t-1", "p2"))).toBe(false);
		expect(sameRun(null, key("t-1"))).toBe(false);
	});

	it("lists a lead's runs latest first, which is the picker's order", () => {
		const view = viewOf(
			run("old", { orchestrators: ["lead"], last_activity: NOW - 30 * MIN }),
			run("new", { orchestrators: ["other", "lead"], last_activity: NOW }),
			run("never", { orchestrators: ["lead"], last_activity: null }),
			run("theirs", { orchestrators: ["other"] })
		);
		expect(runsOf(view, "lead").map((r) => r.key.run)).toEqual(["new", "old", "never"]);
		expect(runsOf(view, "nobody")).toEqual([]);
	});

	it("lists a project's runs that no agent leads", () => {
		const view = viewOf(
			run("bound", { orchestrators: ["lead"] }),
			run("loose", { last_activity: NOW - MIN }),
			run("looser"),
			run("elsewhere", { key: key("elsewhere", "p2") })
		);
		expect(unboundRuns(view, "p1").map((r) => r.key.run)).toEqual(["looser", "loose"]);
		expect(unboundRuns(view, "p2").map((r) => r.key.run)).toEqual(["elsewhere"]);
	});

	it("lists every run in a project, live ones first, each latest first", () => {
		const view = viewOf(
			run("ended", { done: true, last_activity: NOW }),
			run("bound", { orchestrators: ["lead"], last_activity: NOW - 2 * MIN }),
			run("loose", { last_activity: NOW - MIN }),
			run("ended-early", { done: true, last_activity: NOW - MIN }),
			run("elsewhere", { key: key("elsewhere", "p2") })
		);
		expect(projectRuns(view, "p1").map((r) => r.key.run)).toEqual([
			"loose",
			"bound",
			"ended",
			"ended-early",
		]);
	});

	it("gathers the node agents of live runs only", () => {
		const view = viewOf(
			run("live", { agents: ["a", "b"] }),
			run("ended", { agents: ["c"], done: true }),
			run("old-maxd")
		);
		expect([...liveRunAgents(view)]).toEqual(["a", "b"]);
	});

	it("reads a run's state as a glyph's status", () => {
		const status = (over: Partial<WorkflowRun>) => runStatus(run("r", over) as WorkflowRun);
		expect(status({})).toBe("busy");
		expect(status({ needs_you: true })).toBe("waiting_human");
		expect(status({ done: true })).toBe("done");
		expect(status({ done: true, failed: true })).toBe("failed");
	});

	it("finds a run's lead once one binds it", () => {
		const view = viewOf(run("bound", { orchestrators: ["lead"] }), run("loose"));
		expect(runAt(view, key("bound"))?.label).toBe("bound");
		expect(leadOf(view, key("bound"))).toBe("lead");
		expect(leadOf(view, key("loose"))).toBeNull();
		expect(leadOf(view, key("gone"))).toBeNull();
	});

	it("puts the latest run's progress on the lead's chip", () => {
		const view = viewOf(
			run("old", { orchestrators: ["lead"], last_activity: NOW - MIN, done: true }),
			run("new", {
				orchestrators: ["lead"],
				progress: { done: 11, total: 18 },
				needs_you: true,
			})
		);
		expect(chipOf(view, "lead")).toEqual({
			run: key("new"),
			label: "new",
			text: "pipeline 11/18",
			spoken: "new, 11 of 18 done, needs you",
			needsYou: true,
			done: false,
		});
		expect(chipOf(view, "nobody")).toBeNull();
	});
});
