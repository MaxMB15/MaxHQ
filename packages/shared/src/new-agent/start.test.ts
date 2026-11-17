// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { AgentSpec, Entity, PermissionMode } from "../bindings";
import type { ActionResult } from "../data/source";
import { account, device, entities, link, project, workspace } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import { type Choices, emptyChoices, resolve } from "./spec";
import { modeNote, startNewAgent } from "./start";

const settings = (device: string, mode: PermissionMode): Entity => ({
	entity: "device_settings",
	device,
	default_permission: mode,
	workspace_network: null,
	workspace_pool: null,
});

const viewOf = (extra: Entity[] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [device({ id: "dev-mac", name: "studio", os: "macos" })],
				links: [
					link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
				],
				accounts: [
					account({ id: "a-claude" }),
					account({ id: "a-codex", vendor: "codex", label: "Codex" }),
				],
				projects: [project("p1", "MaxHQ")],
				workspaces: [workspace({ id: "ws-mac", project: "p1" })],
			}),
			...extra,
		],
	});

const choose = (over: Partial<Choices>): Choices => ({
	...emptyChoices("p1"),
	workspace: "ws-mac",
	...over,
});

const SPEC: AgentSpec = {
	vendor: "claude",
	account: "a-claude",
	place: { type: "workspace", workspace: "ws-mac" },
	model: null,
	permission: "auto",
	title: null,
	first: null,
	branch: null,
};

const STARTED: ActionResult = { status: "ok", data: { status: "started", agent: "ag-new" } };

describe("the mode note", () => {
	it("names the device's default until you pick a mode", () => {
		const view = viewOf([settings("dev-mac", "plan")]);
		const choices = choose({ account: "a-claude" });
		expect(modeNote(view, choices, resolve(view, choices))).toBe(
			"Plan only is the default on Mac."
		);
	});

	it("says Codex asks first when the default is Plan", () => {
		const view = viewOf([settings("dev-mac", "plan")]);
		const choices = choose({ account: "a-codex" });
		expect(modeNote(view, choices, resolve(view, choices))).toBe(
			"Codex has no plan mode, so it asks first."
		);
	});

	it("says nothing once you pick one", () => {
		const view = viewOf();
		const choices = choose({ account: "a-claude", mode: "ask" });
		expect(modeNote(view, choices, resolve(view, choices))).toBeNull();
	});
});

describe("starting a new agent", () => {
	it("sends the spec as it is when the mode stands", async () => {
		const start = vi.fn(async () => STARTED);
		const before = vi.fn(async () => {});
		const outcome = await startNewAgent(SPEC, "ask", {
			agree: async (mode) => ({ mode, failure: null }),
			before,
			start,
		});
		expect(outcome).toEqual({ agent: "ag-new" });
		expect(before).toHaveBeenCalledWith(SPEC);
		expect(start).toHaveBeenCalledWith(SPEC);
	});

	it("starts in the mode the Auto check settles on", async () => {
		const start = vi.fn(async () => STARTED);
		const agree = vi.fn(async () => ({ mode: "ask" as const, failure: null }));
		await startNewAgent(SPEC, "ask", { agree, start });
		expect(agree).toHaveBeenCalledWith("auto");
		expect(start).toHaveBeenCalledWith({ ...SPEC, permission: "ask" });
	});

	it("asks about the form's mode when the spec names none", async () => {
		const agree = vi.fn(async (mode: PermissionMode) => ({ mode, failure: null }));
		await startNewAgent({ ...SPEC, permission: null }, "plan", {
			agree,
			start: async () => STARTED,
		});
		expect(agree).toHaveBeenCalledWith("plan");
	});

	it("starts nothing when the question closes or fails", async () => {
		const start = vi.fn(async () => STARTED);
		expect(
			await startNewAgent(SPEC, "ask", {
				agree: async () => ({ mode: null, failure: null }),
				start,
			})
		).toEqual({ failure: null });
		expect(
			await startNewAgent(SPEC, "ask", {
				agree: async () => ({ mode: null, failure: "Couldn't switch it" }),
				start,
			})
		).toEqual({ failure: "Couldn't switch it" });
		expect(start).not.toHaveBeenCalled();
	});

	it("says why maxd didn't start it", async () => {
		const outcome = await startNewAgent(SPEC, "ask", {
			agree: async (mode) => ({ mode, failure: null }),
			start: async () => ({
				status: "ok",
				data: { status: "refused", reason: "Codex is at its limit" },
			}),
		});
		expect(outcome).toEqual({ failure: "Codex is at its limit" });
	});

	it("says to try again when a step throws", async () => {
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		const outcome = await startNewAgent(SPEC, "ask", {
			agree: async (mode) => ({ mode, failure: null }),
			start: async () => {
				throw new Error("gone");
			},
		});
		expect(outcome).toEqual({ failure: "It didn't start. Try again." });
		warn.mockRestore();
	});
});
