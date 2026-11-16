// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "../bindings";
import { mockTasks } from "../data/mock-tasks";
import { DEVBOX, MAC, mockEntities } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	agentOptions,
	alwaysOn,
	defaultHome,
	deviceTasks,
	githubRepos,
	homeOf,
	keptByAutomation,
	sleepWarning,
	taskDevices,
	tasksTargeting,
} from "./place";

const tasks = mockTasks({ now: () => NOW }).entities();

/** The sample view, with each link changed by `edit`. */
const viewWith = (edit: (e: Entity) => Entity = (e) => e) =>
	applySnapshot({ seq: 1, entities: [...mockEntities(NOW), ...tasks].map(edit) });

const view = viewWith();

/** The sample view where one device's `maxd` predates scheduled tasks. */
const older = (device: string) =>
	viewWith((e): Entity =>
		e.entity === "device_link" && e.device === device
			? {
					...e,
					capabilities: e.capabilities && { ...e.capabilities, scheduled_tasks: false },
				}
			: e
	);

describe("the devices that run tasks", () => {
	it("lists each with its tasks and Funnel", () => {
		expect(taskDevices(view)).toEqual([DEVBOX, MAC]);
		const byDevice = Object.fromEntries(deviceTasks(view).map((d) => [d.device, d]));
		expect(byDevice[MAC]!.tasks.map((t) => t.id)).toEqual([
			"task-morning",
			"task-checklist",
			"task-deploy",
		]);
		expect(byDevice[MAC]!.funnel.state).toBe("needs_setup");
		expect(byDevice[DEVBOX]!.funnel.state).toBe("on");
	});

	it("hides a device whose maxd is older", () => {
		const view = older(DEVBOX);
		expect(taskDevices(view)).toEqual([MAC]);
		expect(homeOf(view, "task-triage")).toBeUndefined();
		expect(githubRepos(view)).toEqual([]);
	});
});

describe("a new task's home", () => {
	it("is the target's device for a timer", () => {
		expect(defaultHome(view, "every", MAC)).toBe(MAC);
		expect(defaultHome(view, "at", null)).toBe(DEVBOX);
	});

	it("is the device that stays on for a webhook", () => {
		expect(alwaysOn(view)).toBe(DEVBOX);
		expect(defaultHome(view, "hook", MAC)).toBe(DEVBOX);
		expect(defaultHome(older(DEVBOX), "hook", MAC)).toBe(MAC);
	});

	it("is a device whose App reaches the repo for GitHub events", () => {
		expect(githubRepos(view)).toEqual([{ repo: "Example-User/MaxHQ", device: DEVBOX }]);
		expect(defaultHome(view, "github_events", MAC, "Example-User/MaxHQ")).toBe(DEVBOX);
	});

	it("warns when a timed task lives on a Mac", () => {
		expect(sleepWarning(view, "at", MAC)).toMatch(/doesn't wake up to run it/);
		expect(sleepWarning(view, "at", DEVBOX)).toBeNull();
		expect(sleepWarning(view, "hook", MAC)).toBeNull();
	});
});

describe("the agents a task reaches", () => {
	it("offers the agents MaxHQ holds", () => {
		const values = agentOptions(view).map((o) => o.value);
		expect(values).toContain("agent-release");
		expect(values.every((v) => view.tables.agent.get(v)?.control === "held")).toBe(true);
	});

	it("keeps the agent a task reaches already, though MaxHQ doesn't hold it", () => {
		expect(agentOptions(view).map((o) => o.value)).not.toContain("agent-gtfs");
		const kept = agentOptions(view, "agent-gtfs").find((o) => o.value === "agent-gtfs");
		expect(kept).toMatchObject({ label: "GTFS feed refresh", detail: "devbox" });
	});

	it("finds the tasks that target an agent, with their home", () => {
		expect(tasksTargeting(view, "agent-release")).toEqual([
			{ task: expect.objectContaining({ id: "task-checklist" }), home: MAC },
			{ task: expect.objectContaining({ id: "task-deploy" }), home: MAC },
		]);
		expect(tasksTargeting(view, "agent-nobody")).toEqual([]);
	});

	it("keeps an agent active while one of its tasks is enabled", () => {
		expect(keptByAutomation(view, "agent-release")).toBe(true);
		expect(keptByAutomation(view, "agent-nobody")).toBe(false);
		const paused = viewWith((e): Entity =>
			e.entity === "device_tasks"
				? { ...e, tasks: e.tasks.map((task) => ({ ...task, enabled: false })) }
				: e
		);
		expect(keptByAutomation(paused, "agent-release")).toBe(false);
	});
});
