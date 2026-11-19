// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { Task } from "../bindings";
import type { ActionResult } from "../data/source";
import { mockEntities } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { saveDraft, taskActions } from "./actions";
import { emptyDraft, type TaskDraft } from "./draft";

const view = applySnapshot({ seq: 1, entities: mockEntities(NOW) });
const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const hookDraft = (over: Partial<TaskDraft> = {}): TaskDraft => ({
	...emptyDraft("Europe/Amsterdam"),
	name: "Deploys",
	prompt: "Deploy {{body.ref}}",
	kind: "hook",
	target: "agent",
	agent: "agent-release",
	signed: true,
	secret: " s3cret ",
	...over,
});

const calls = () => {
	const saved: Task[] = [];
	const source = {
		saveTask: vi.fn(async (_device: string, task: Task): Promise<ActionResult> => {
			saved.push(task);
			return {
				status: "ok",
				data: { status: "task_saved", task, hook_url: "https://hooks.example.com/x" },
			};
		}),
		setHookSecret: vi.fn(async (): Promise<ActionResult> => DONE),
		setHookOn: vi.fn(async (): Promise<ActionResult> => DONE),
	};
	return { source, saved };
};

describe("saveDraft", () => {
	it("refuses a draft with no name or no device, and sends nothing", async () => {
		const { source } = calls();
		expect(await saveDraft(source, view, hookDraft({ name: " " }), "mac", true, "t1")).toEqual({
			state: "refused",
			reason: "Give it a name.",
		});
		expect(await saveDraft(source, view, hookDraft(), null, true, "t1")).toEqual({
			state: "refused",
			reason: "Pick the device it lives on.",
		});
		expect(source.saveTask).not.toHaveBeenCalled();
	});

	it("sends the secret on its own call, turns the new webhook on and shows its URL", async () => {
		const { source, saved } = calls();
		const result = await saveDraft(source, view, hookDraft(), "mac", true, "t1");
		expect(result).toMatchObject({ state: "saved", hookUrl: "https://hooks.example.com/x" });
		expect(JSON.stringify(saved)).not.toContain("s3cret");
		expect(source.setHookSecret).toHaveBeenCalledWith("t1", "s3cret");
		expect(source.setHookOn).toHaveBeenCalledWith("t1", true);
	});

	it("leaves a new webhook off when asked, and says what didn't go through without the secret", async () => {
		const { source } = calls();
		source.setHookSecret.mockResolvedValueOnce(refused("Bad secret s3cret"));
		const result = await saveDraft(source, view, hookDraft(), "mac", false, "t1");
		expect(source.setHookOn).not.toHaveBeenCalled();
		expect(result).toMatchObject({
			state: "partly",
			reason: "Saved, but Bad secret <secret>",
		});
	});

	it("says the device's refusal, and asks to try again when the call fails", async () => {
		const { source } = calls();
		source.saveTask.mockResolvedValueOnce(refused("No room"));
		expect(await saveDraft(source, view, hookDraft(), "mac", true, "t1")).toEqual({
			state: "refused",
			reason: "No room",
		});
		source.saveTask.mockRejectedValueOnce(new Error("gone"));
		const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
		expect(await saveDraft(source, view, hookDraft(), "mac", true, "t1")).toEqual({
			state: "refused",
			reason: "Couldn't save it. Try again.",
		});
		warn.mockRestore();
	});
});

describe("taskActions", () => {
	const task = { id: "t1" } as Task;
	const source = () => ({
		setTaskEnabled: vi.fn(async (): Promise<ActionResult> => DONE),
		runTaskNow: vi.fn(async (): Promise<ActionResult> => DONE),
		rotateHookToken: vi.fn(async (): Promise<ActionResult> => ({
			status: "ok",
			data: { status: "hook_url", url: "https://hooks.example.com/y" },
		})),
		deleteTask: vi.fn(async (): Promise<ActionResult> => DONE),
	});

	it("reads the runs again after a run, and hands over a rotated URL", async () => {
		const calls = source();
		const ran = vi.fn();
		const actions = taskActions(calls, ran);
		expect(await actions.runNow(task)).toBeNull();
		expect(ran).toHaveBeenCalledWith("t1");
		const show = vi.fn();
		expect(await actions.rotate(task, show)).toBeNull();
		expect(show).toHaveBeenCalledWith("https://hooks.example.com/y");
		expect(await actions.setEnabled(task, "mac", false)).toBeNull();
		expect(calls.setTaskEnabled).toHaveBeenCalledWith("mac", "t1", false);
	});

	it("says why a call didn't go through", async () => {
		const calls = source();
		calls.runTaskNow.mockResolvedValueOnce(refused("Paused"));
		calls.deleteTask.mockRejectedValueOnce(new Error("gone"));
		calls.setTaskEnabled.mockRejectedValueOnce(new Error("gone"));
		const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
		const ran = vi.fn();
		const actions = taskActions(calls, ran);
		expect(await actions.runNow(task)).toBe("Paused");
		expect(ran).not.toHaveBeenCalled();
		expect(await actions.remove(task)).toBe("Couldn't delete it. Try again.");
		expect(await actions.setEnabled(task, "mac", true)).toBe("Couldn't turn it on. Try again.");
		warn.mockRestore();
	});
});
