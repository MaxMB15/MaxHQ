// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Agent, Entity, ModelLoad } from "../bindings";
import { MAC_LOCAL_AGENT, MAC_SERVER } from "../data/mock-local-models";
import { mockSource } from "../data/mock-source";
import { agent, NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import { localLoading, localSpeed, turnRuns } from "./running";

/** gpt-oss-20b's ID on the Mac's managed server. */
const GPT_OSS = "gpt-oss-20b@mxfp4";

/** The sample view, with the Mac's gpt-oss-20b in `load`. */
const load = async (gptOss: ModelLoad = { state: "unloaded" }): Promise<ViewState> => {
	const snapshot = await mockSource({ now: () => NOW, tick: false }).snapshot();
	const entities = snapshot.entities.map((e): Entity =>
		e.entity === "model_server" && e.id === MAC_SERVER
			? {
					...e,
					models: e.models.map((m) => (m.id === GPT_OSS ? { ...m, load: gptOss } : m)),
				}
			: e
	);
	return applySnapshot({ ...snapshot, entities });
};

const parser = (view: ViewState, over: Partial<Agent> = {}): Agent => ({
	...view.tables.agent.get(MAC_LOCAL_AGENT)!,
	...over,
});

describe("Loading model", () => {
	it("shows while a turn runs on a model that isn't in memory", async () => {
		const view = await load();
		const cold = parser(view, { model: GPT_OSS });
		expect(localLoading(view, cold)).toEqual({ model: "gpt-oss-20b", percent: null });
		expect(localLoading(view, { ...cold, activity: { state: "starting" } })).toEqual({
			model: "gpt-oss-20b",
			percent: null,
		});
		// Not once the turn ends, nor for a model in memory.
		expect(localLoading(view, { ...cold, activity: { state: "idle" } })).toBeNull();
		expect(localLoading(view, parser(view))).toBeNull();
	});

	it("shows while the server loads the model, and when the agent says so", async () => {
		const view = await load({ state: "loading" });
		expect(localLoading(view, parser(view, { model: GPT_OSS }))).toEqual({
			model: "gpt-oss-20b",
			percent: null,
		});
		const idle = parser(view, {
			activity: { state: "idle" },
			local: { loading: true },
		});
		expect(localLoading(view, idle)).toEqual({ model: "Qwen3.6-35B-A3B", percent: null });
	});

	it("carries how far the server got, up to 99 while it warms up", async () => {
		for (const percent of [42, 99]) {
			const view = await load({ state: "loading", percent });
			expect(localLoading(view, parser(view, { model: GPT_OSS }))).toEqual({
				model: "gpt-oss-20b",
				percent,
			});
		}
	});

	it("never shows for an agent that isn't local", async () => {
		const view = await load();
		const claude = agent({ id: "c", model: "gpt-oss-20b", activity: { state: "busy" } });
		expect(localLoading(view, claude)).toBeNull();
	});
});

describe("the speed line", () => {
	it("reads the prompt and writing speeds and the context while the turn runs", async () => {
		const view = await load();
		expect(localSpeed(parser(view))).toEqual({
			text: "1,180 / 74 tok/s · 31k of 66k",
			short: "74 tok/s",
			full: "Reads the prompt at 1,180 tokens/s · Writes 74 tokens/s · Context 31k of 66k in use",
			live: true,
		});
	});

	it("keeps the last turn's numbers after it, as the last turn", async () => {
		const view = await load();
		const done = localSpeed(parser(view, { activity: { state: "done_unread" } }))!;
		expect(done.live).toBe(false);
		expect(done.text).toBe("1,180 / 74 tok/s · 31k of 66k");
		expect(done.full).toMatch(/^Last turn: Reads the prompt at 1,180 tokens\/s/);
	});

	it("shows what the server reported, and nothing before it reports", () => {
		const only = (local: Agent["local"]) =>
			localSpeed(agent({ id: "x", activity: { state: "busy" }, local }));
		expect(only({ decode_per_second: 61.6 })).toMatchObject({
			text: "62 tok/s",
			short: "62 tok/s",
		});
		expect(only({ context_used: 900, context_size: 4096 })).toMatchObject({
			text: "900 of 4k",
			short: "900 of 4k",
		});
		expect(only({ loading: true })).toBeNull();
		expect(only(null)).toBeNull();
	});

	it("counts a starting turn as running", () => {
		expect(turnRuns({ activity: { state: "starting" } })).toBe(true);
		expect(turnRuns({ activity: { state: "busy_in_subagents" } })).toBe(false);
		expect(turnRuns({ activity: { state: "done_unread" } })).toBe(false);
	});
});
