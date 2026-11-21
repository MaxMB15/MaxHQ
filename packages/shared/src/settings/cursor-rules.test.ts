// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ToolInfo } from "../bindings";
import { mockCursorRules, noCursorRules } from "../data/mock-cursor-rules";
import { MAC } from "../data/mock-view";
import type { Ask } from "../data/request-source";
import { device, entities, link } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	afterRemove,
	cursorRuleCalls,
	cursorRuleDevices,
	type CursorRuleState,
	ruleState,
	ruleWords,
} from "./cursor-rules";

const PATH = "~/.cursor/cli-config.json";

const listed = (rules: string[]): CursorRuleState => ({ kind: "listed", path: PATH, rules });

const cli = (tool: "claude" | "cursor", path: string | null): ToolInfo => ({
	kind: { type: "agent", tool },
	place: { type: "host" },
	path,
	version: path ? "1.0.0" : null,
	latest: null,
	owner: "user",
	method: "native",
	update: { type: "needs_tick" },
	state: { type: "idle" },
});

describe("the shell rules Cursor saved", () => {
	it("asks the device named for its list or a removal", async () => {
		const asked: unknown[] = [];
		const ask: Ask = async (request) => {
			asked.push(request);
			return { status: "ok", data: { status: "too_old" } } as never;
		};
		const calls = cursorRuleCalls(ask);
		await calls.cursorRules("dev-devbox", { kind: "list" });
		await calls.cursorRules("dev-devbox", { kind: "remove", rule: "Shell(ls)" });
		expect(asked).toEqual([
			{ kind: "cursor_rules", device: "dev-devbox", call: { kind: "list" } },
			{
				kind: "cursor_rules",
				device: "dev-devbox",
				call: { kind: "remove", rule: "Shell(ls)" },
			},
		]);
	});

	it("shows a device's list, an old maxd, or why it failed", () => {
		expect(
			ruleState({
				status: "ok",
				data: { status: "listed", rules: { path: PATH, rules: ["Shell(ls)"] } },
			})
		).toEqual(listed(["Shell(ls)"]));
		expect(ruleState({ status: "ok", data: { status: "too_old" } })).toEqual({
			kind: "too_old",
		});
		expect(
			ruleState({
				status: "error",
				error: { kind: "failed", message: "Cursor's settings aren't JSON" },
			})
		).toEqual({ kind: "failed", message: "Cursor's settings aren't JSON" });
		expect(
			ruleState({ status: "error", error: { kind: "not_connected", device: "devbox" } })
		).toEqual({
			kind: "failed",
			message: "The device isn't connected, so nothing changed",
		});
	});

	it("keeps the list as it was when a removal fails, and says why", () => {
		const before = listed(["Shell(ls)", "Shell(git status)"]);
		expect(afterRemove(before, listed(["Shell(ls)"]))).toEqual({
			state: listed(["Shell(ls)"]),
			error: null,
		});
		expect(afterRemove(before, { kind: "failed", message: "Couldn't write it" })).toEqual({
			state: before,
			error: "Couldn't write it",
		});
	});

	it("shows the words a rule lets through", () => {
		expect(ruleWords("Shell(git status)")).toBe("git status");
		expect(ruleWords("Shell(ls)")).toBe("ls");
		expect(ruleWords("Shell(")).toBe("Shell(");
		expect(ruleWords("Read(src/**)")).toBe("Read(src/**)");
	});

	it("has a part for each linked device unless it has no Cursor CLI", () => {
		const view = applySnapshot({
			seq: 1,
			entities: [
				...entities({
					links: [
						link({
							id: "mac",
							name: "Mac",
							device: "dev-mac",
							route: { type: "local" },
						}),
						link({ id: "devbox", device: "dev-devbox" }),
						link({ id: "alpha", device: "dev-alpha" }),
						link({ id: "beta", device: "dev-beta" }),
					],
					devices: [
						device({ id: "dev-mac", name: "Mac", os: "macos" }),
						device({ id: "dev-devbox", name: "devbox" }),
						device({ id: "dev-alpha", name: "alpha" }),
						device({ id: "dev-beta", name: "beta" }),
					],
				}),
				{
					entity: "device_tools",
					device: "dev-mac",
					tools: [cli("cursor", "/Users/me/.local/bin/agent")],
				},
				// Only Claude Code, and a Cursor CLI that isn't there.
				{
					entity: "device_tools",
					device: "dev-alpha",
					tools: [cli("claude", "/usr/local/bin/claude"), cli("cursor", null)],
				},
				// No CLIs at all. devbox hasn't said what it has yet, so it
				// keeps its part.
				{ entity: "device_tools", device: "dev-beta", tools: [] },
			],
		});
		expect(cursorRuleDevices(view)).toEqual([
			{ device: "dev-mac", name: "Mac" },
			{ device: "dev-devbox", name: "devbox" },
		]);
	});

	it("removes a sample rule and keeps the rest, and refuses another kind", async () => {
		const mock = mockCursorRules();
		expect(await mock.cursorRules(MAC, { kind: "remove", rule: "Shell(pnpm test)" })).toEqual({
			status: "ok",
			data: {
				status: "listed",
				rules: { path: PATH, rules: ["Shell(git status)", "Shell(cargo build)"] },
			},
		});
		expect(await mock.cursorRules(MAC, { kind: "remove", rule: "Read(src/**)" })).toEqual({
			status: "error",
			error: { kind: "failed", message: "maxd removes only Shell(...) rules." },
		});
		expect(await mock.cursorRules("dev-devbox", { kind: "list" })).toEqual({
			status: "ok",
			data: { status: "too_old" },
		});
		expect(await noCursorRules.cursorRules(MAC, { kind: "list" })).toEqual({
			status: "ok",
			data: { status: "too_old" },
		});
	});
});
