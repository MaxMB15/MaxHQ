// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { WorkflowUpdate } from "../bindings";
import { NOW } from "../test/build";
import { all, applySnapshot } from "../view/replica";
import { runsOf, unboundRuns } from "../workflow/select";
import { mockSource } from "./mock-source";
import { DEVBOX } from "./mock-view";
import { EXAMPLE_API, EXAMPLE_APP, RUN_LEADS } from "./mock-workflow";

const source = (scenario?: string) => mockSource({ now: () => NOW, tick: false, scenario });

const load = async (scenario?: string) => applySnapshot(await source(scenario).snapshot());

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("the sample workflow runs", () => {
	it("has eight runs with placeholder names only", async () => {
		const runs = all(await load(), "workflow_run");
		expect(runs).toHaveLength(8);
		for (const run of runs) {
			expect(run.key.run).toMatch(/^t-20261101-example-|^agent-ex-/);
			expect([EXAMPLE_APP, EXAMPLE_API]).toContain(run.key.project);
		}
	});

	it("gives each lead its run, and leaves one on example-api without a lead", async () => {
		const view = await load();
		for (const lead of Object.values(RUN_LEADS)) expect(runsOf(view, lead)).toHaveLength(1);
		expect(unboundRuns(view, EXAMPLE_API).map((r) => r.label)).toEqual(["Webhook retries"]);
		expect(unboundRuns(view, EXAMPLE_APP)).toEqual([]);
	});

	it("says which runs wait on you", async () => {
		const waiting = all(await load(), "workflow_run").filter((r) => r.needs_you);
		expect(waiting.map((r) => r.label).sort()).toEqual([
			"Rate limit design",
			"Settings page redesign",
		]);
	});

	it("greys only the Codex run's agents when devbox is offline", async () => {
		const view = await load("peer_offline");
		const [codex] = runsOf(view, RUN_LEADS.codex);
		expect(codex?.key.device).toBe(DEVBOX);
		expect(view.tables.agent.get(RUN_LEADS.codex)?.activity.state).toBe("not_accessible");
		expect(view.tables.agent.get(RUN_LEADS.analysis)?.activity.state).not.toBe(
			"not_accessible"
		);
		expect(all(view, "workflow_run")).toEqual(all(await load(), "workflow_run"));
		expect(all(await load("tailscale_off"), "workflow_run")).toEqual(
			all(await load(), "workflow_run")
		);
	});

	it("opens a run with its snapshot as a reset", async () => {
		const mock = source();
		const [run] = runsOf(await load(), RUN_LEADS.loops);
		const updates: WorkflowUpdate[] = [];
		const stop = await mock.openWorkflow(run!.key, (u) => updates.push(u));
		await settle();
		expect(updates).toHaveLength(1);
		const [first] = updates;
		if (first?.kind !== "reset") throw new Error("expected a reset");
		expect(first.run.header).toMatchObject({ label: "Offline sync", status: "done" });
		expect(first.run.loops.map((l) => l.label)).toEqual(["integrate", "re-verify", "CI red"]);
		stop();
	});

	it("turns down a run it doesn't have", async () => {
		const key = { device: DEVBOX, project: EXAMPLE_APP, workflow: "pipeline", run: "gone" };
		await expect(source().openWorkflow(key, () => {})).rejects.toThrow();
	});

	it("shows a node's lists, and says when the node isn't there", async () => {
		const mock = source();
		const [run] = runsOf(await load(), RUN_LEADS.codex);
		const detail = await mock.nodeDetail(run!.key, "design");
		expect(detail.status).toBe("ok");
		if (detail.status === "ok") {
			expect(detail.data.lists.map((l) => l.label)).toContain("Notes");
		}
		expect(await mock.nodeDetail(run!.key, "nowhere")).toMatchObject({
			status: "error",
			error: { kind: "not_found" },
		});
	});
});
