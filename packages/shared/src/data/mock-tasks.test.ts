// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { DeviceTasks, Task } from "../bindings";
import { NOW } from "../test/build";
import { mockTasks } from "./mock-tasks";
import { DEVBOX, MAC } from "./mock-view";

const RULES = {
	filters: [],
	replay_key: null,
	coalesce_key: null,
	coalesce_secs: null,
	max_age_minutes: null,
	daily_runs: null,
};

const hookTask: Task = {
	id: "task-new",
	name: "New hook",
	prompt: "Handle it",
	trigger: { type: "hook", on: false, signature: null, rules: RULES, hold: false },
	target: { type: "agent", device: MAC, agent: "agent-release" },
	enabled: true,
	permission: "",
	made_by: { type: "user" },
	next_run: null,
	last_run: null,
};

const setup = () => {
	const changed = vi.fn();
	const mock = mockTasks({ now: () => NOW, changed });
	const on = (device: string) =>
		mock.entities().find((e) => e.entity === "device_tasks" && e.device === device) as
			(DeviceTasks & { entity: "device_tasks" }) | undefined;
	return { ...mock, changed, on };
};

describe("the sample tasks", () => {
	it("answers a new hook task's URL once", async () => {
		const { calls, changed, on } = setup();
		const first = await calls.saveTask(MAC, hookTask);
		expect(first).toMatchObject({
			status: "ok",
			data: { status: "task_saved", task: { permission: "perm-task-new" } },
		});
		const url =
			first.status === "ok" && first.data.status === "task_saved" && first.data.hook_url;
		expect(url).toMatch(
			/^https:\/\/macbook-pro\.tail1234\.ts\.net\/maxhq\/task-new\/[\w-]{43}$/
		);
		const again = await calls.saveTask(MAC, { ...hookTask, name: "Renamed" });
		expect(again).toMatchObject({ data: { hook_url: null } });
		expect(changed).toHaveBeenCalledTimes(2);
		expect(on(MAC)!.tasks.find((t) => t.id === "task-new")!.name).toBe("Renamed");
	});

	it("turns a task on or off only on the device that holds it", async () => {
		const { calls, on } = setup();
		const task = on(MAC)!.tasks[0]!;
		expect(await calls.setTaskEnabled(DEVBOX, task.id, false)).toMatchObject({
			status: "error",
		});
		await calls.setTaskEnabled(MAC, task.id, false);
		expect(on(MAC)!.tasks[0]).toEqual({ ...task, enabled: false, next_run: null });
	});

	it("keeps whether a secret is held, which only the device sets", async () => {
		const { calls, on } = setup();
		const triage = on(DEVBOX)!.tasks.find((t) => t.id === "task-triage")!;
		if (triage.trigger.type !== "hook") throw new Error("not a hook");
		const forged = {
			...triage,
			trigger: {
				...triage.trigger,
				signature: { ...triage.trigger.signature!, has_secret: false },
			},
		};
		await calls.saveTask(DEVBOX, forged);
		expect(on(DEVBOX)!.tasks.find((t) => t.id === "task-triage")!.trigger).toMatchObject({
			signature: { has_secret: true },
		});
		expect(await calls.setHookSecret("task-deploy", " ")).toMatchObject({ status: "error" });
		await calls.setHookSecret("task-deploy", "s3cret");
		expect(on(MAC)!.tasks.find((t) => t.id === "task-deploy")!.trigger).toMatchObject({
			signature: { has_secret: true },
		});
	});

	it("turns the Funnel off when no webhook listens", async () => {
		const { calls, on } = setup();
		expect(on(MAC)!.funnel.state).toBe("needs_setup");
		await calls.setHookOn("task-deploy", false);
		expect(on(MAC)!.funnel).toEqual({ state: "off" });
		expect(await calls.funnelState(MAC)).toEqual({ status: "ok", data: { state: "off" } });
	});

	it("moves a device's hook URLs to the address it picks", async () => {
		const { calls, on } = setup();
		expect(on(DEVBOX)!.address).toEqual({ kind: "funnel" });
		await calls.setHookAddress(DEVBOX, { kind: "own", base: "https://hooks.example.com/" });
		expect(on(DEVBOX)!.address).toEqual({ kind: "own", base: "https://hooks.example.com" });
		expect(on(DEVBOX)!.funnel).toEqual({ state: "on", url: "https://hooks.example.com" });
		expect(await calls.rotateHookToken("task-triage")).toMatchObject({
			data: {
				url: expect.stringMatching(/^https:\/\/hooks\.example\.com\/maxhq\/task-triage\//),
			},
		});
		await calls.setHookAddress(MAC, { kind: "local" });
		expect(on(MAC)!.funnel).toEqual({ state: "on", url: "http://127.0.0.1:41234" });
	});

	it("runs a task now, and lists its runs and requests", async () => {
		const { calls, on } = setup();
		await calls.runTaskNow("task-checklist");
		expect(on(MAC)!.tasks.find((t) => t.id === "task-checklist")!.last_run).toEqual({
			at: NOW,
			outcome: "running",
		});
		const runs = await calls.listRuns("task-checklist");
		expect(runs.status === "ok" && runs.data[0]).toMatchObject({
			cause: "now",
			outcome: "running",
			agent: "agent-release",
		});
		const deliveries = await calls.listDeliveries("task-triage");
		expect(deliveries.status === "ok" && deliveries.data.length).toBe(4);
	});

	it("rotates a hook's URL, and deletes a task", async () => {
		const { calls, on } = setup();
		expect(await calls.rotateHookToken("task-triage")).toMatchObject({
			data: {
				status: "hook_url",
				url: expect.stringContaining("devbox.tail1234.ts.net/maxhq/task-triage/"),
			},
		});
		expect(await calls.rotateHookToken("task-morning")).toMatchObject({ status: "error" });
		await calls.deleteTask("task-triage");
		expect(on(DEVBOX)!.tasks.map((t) => t.id)).toEqual(["task-reviews"]);
		expect(await calls.listRuns("task-triage")).toMatchObject({
			status: "error",
			error: { kind: "not_found" },
		});
	});
});
