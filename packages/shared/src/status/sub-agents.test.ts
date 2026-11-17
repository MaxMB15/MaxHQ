// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Activity } from "../bindings";
import { mockSource } from "../data/mock-source";
import { agent, NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import { contextShare, leadStatus, subAgentsSubtitle } from "./sub-agents";

const load = async (): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());

describe("sub-agent list", () => {
	it("says whose login the lead runs on, where and how many sub-agents it has", async () => {
		const view = await load();
		const lead = [...view.tables.agent.values()].find(
			(a) => a.title === "M1 observe milestone"
		)!;
		expect(subAgentsSubtitle(view, lead, 7)).toBe("Claude Max on Mac, 7 sub-agents");
		expect(subAgentsSubtitle(view, lead, 1)).toBe("Claude Max on Mac, 1 sub-agent");
	});

	it("says what the lead itself is doing", () => {
		const cases: [Activity, string][] = [
			[{ state: "busy_in_subagents" }, "Waiting"],
			[{ state: "done_unread" }, "Idle"],
			[{ state: "idle_cache_warm", warm_until: NOW }, "Idle"],
			[{ state: "busy" }, "Working"],
			[{ state: "needs_input", attention: "att-1" }, "Needs input"],
		];
		for (const [activity, text] of cases) expect(leadStatus(activity)).toBe(text);
	});

	it("says how full the context window is, capped at all of it", () => {
		const context = (used: number, window: number) =>
			agent({ id: "lead", context: { used_tokens: used, window_tokens: window } }).context;
		expect(contextShare(context(50_000, 200_000))).toBe(25);
		expect(contextShare(context(300_000, 200_000))).toBe(100);
		expect(contextShare(context(10, 0))).toBeNull();
		expect(contextShare(null)).toBeNull();
	});
});
