// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { DISPATCHER_HOME } from "./mock-dispatcher";
import { mockSource } from "./mock-source";
import { CLAUDE, CODEX, CURSOR, MAC } from "./mock-view";

const REQUEST = "Review octocat's new PRs, 15 minutes after the last one";

const setup = () => {
	const mock = mockSource({ now: () => NOW, tick: false });
	const view = async () => applySnapshot(await mock.snapshot());
	const dispatcher = async () => (await view()).tables.dispatcher.get(MAC)!;
	/** Starts a conversation and answers its question with `project`. */
	const planFor = async (project: string) => {
		await mock.askDispatcher(MAC, REQUEST, CLAUDE, false);
		const agent = (await view()).tables.agent.get((await dispatcher()).agent!)!;
		if (agent.activity.state !== "needs_input") throw new Error("No question");
		await mock.answer(agent.activity.attention, { type: "option", id: project });
		return (await dispatcher()).plan!;
	};
	return { mock, view, dispatcher, planFor };
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("the sample dispatcher", () => {
	it("starts a conversation in its own folder that asks which project", async () => {
		const { mock, view, dispatcher } = setup();
		const asked = await mock.askDispatcher(MAC, REQUEST, CLAUDE, false);
		expect(asked).toMatchObject({ status: "ok", data: { status: "started" } });
		const state = await view();
		const agent = state.tables.agent.get((await dispatcher()).agent!)!;
		expect(agent).toMatchObject({ title: "Dispatcher", workspace: DISPATCHER_HOME });
		expect(agent.permission?.requested).toBe("ask");
		if (agent.activity.state !== "needs_input") throw new Error("No question");
		const question = state.tables.attention.get(agent.activity.attention)!;
		expect(question.options.map((o) => o.label)).toEqual(
			expect.arrayContaining(["Comet", "MaxHQ"])
		);
	});

	it("plans for the project you pick and runs nothing yet", async () => {
		const { view, planFor } = setup();
		const plan = await planFor("Comet");
		expect(plan.items.map((i) => i.step.type)).toEqual(["start_agent", "schedule_task"]);
		const task = plan.items[1]!.step;
		if (task.type !== "schedule_task" || task.trigger.type !== "github_events")
			throw new Error("No task");
		expect(task.trigger.repo).toBe("Example-User/Comet");
		expect(task.trigger.rules).toMatchObject({ coalesce_secs: 900, coalesce_restart: true });
		const agents = [...(await view()).tables.agent.values()];
		expect(agents.some((a) => a.title === "PR reviews")).toBe(false);
	});

	it("runs the plan you confirm, the task aimed at the agent it starts", async () => {
		const { mock, view, dispatcher, planFor } = setup();
		const plan = await planFor("Comet");
		const result = await mock.confirmPlan(MAC, plan.id);
		if (result.status !== "ok" || result.data.status !== "plan_ran") throw new Error("Not run");
		const [started, saved] = result.data.results;
		if (started?.type !== "started" || saved?.type !== "task_saved") throw new Error("Failed");
		const state = await view();
		expect(state.tables.agent.get(started.agent)?.vendor).toBe("cursor");
		const tasks = await mock.listTasks(MAC);
		const task =
			tasks.status === "ok" ? tasks.data.find((t) => t.id === saved.task) : undefined;
		expect(task?.target).toEqual({ type: "agent", device: MAC, agent: started.agent });
		expect((await dispatcher()).plan).toBeNull();
	});

	it("drops a plan you dismiss, and refuses one no longer shown", async () => {
		const { mock, dispatcher, planFor } = setup();
		const plan = await planFor("MaxHQ");
		const stale = {
			status: "ok",
			data: { status: "refused", reason: "That plan isn't the one shown any more." },
		};
		expect(await mock.confirmPlan(MAC, plan.id + 1)).toEqual(stale);
		expect(await mock.dismissPlan(MAC, plan.id)).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		expect((await dispatcher()).plan).toBeNull();
		expect(await mock.confirmPlan(MAC, plan.id)).toEqual(stale);
	});

	it("starts over for a new conversation or another account", async () => {
		const { mock, view, dispatcher, planFor } = setup();
		await planFor("Comet");
		const first = (await dispatcher()).agent!;
		await mock.askDispatcher(MAC, "And another", CODEX, false);
		const second = await dispatcher();
		expect(second.agent).not.toBe(first);
		expect(second.plan).toBeNull();
		expect((await view()).tables.agent.has(first)).toBe(false);
	});

	it("refuses what the real one refuses", async () => {
		const { mock } = setup();
		const refused = (reason: string) => ({ status: "ok", data: { status: "refused", reason } });
		expect(await mock.askDispatcher(MAC, "  ", CLAUDE, false)).toEqual(
			refused("Type a request first.")
		);
		expect(await mock.askDispatcher(MAC, REQUEST, CURSOR, false)).toEqual(
			refused("The dispatcher runs as a Claude or Codex account on this Mac.")
		);
		expect(await mock.askDispatcher("dev-devbox", REQUEST, CLAUDE, false)).toEqual(
			refused("The dispatcher runs on the Mac.")
		);
	});
});
