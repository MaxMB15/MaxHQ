// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { handoffKinds, handoffLine } from "../agent-view/format";
import type { ItemBody, TimelineItem } from "../bindings";
import { LOADED_MODEL } from "../data/mock-local-models";
import { mockSource } from "../data/mock-source";
import { NOW } from "../test/build";
import { timelineItem } from "../timeline/build";
import { applySnapshot } from "../view/replica";

import { handoffEndText } from "./handoff";

type Handoff = Extract<ItemBody, { kind: "handoff" }>;

const move = (over: Partial<Handoff>): Extract<TimelineItem, { kind: "handoff" }> =>
	timelineItem(
		{
			kind: "handoff",
			from_vendor: "claude",
			from_model: null,
			to_vendor: "claude",
			to_model: null,
			from_local: false,
			to_local: false,
			...over,
		},
		{ id: "move" }
	) as Extract<TimelineItem, { kind: "handoff" }>;

describe("a move's ends", () => {
	it("name an end Local from the move's flags, never from its model", async () => {
		const view = applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());
		const local = handoffKinds(move({ from_local: true, from_model: LOADED_MODEL }));
		expect(local).toEqual({ from: "local", to: "claude" });
		expect(handoffEndText(view, local.from, LOADED_MODEL)).toBe("Local · Qwen3.6-35B-A3B");
		expect(handoffEndText(view, "local", "qwen3-coder:30b")).toBe("Local · qwen3-coder:30b");
		expect(handoffEndText(view, "local", null)).toBe("Local");
		// A model a local server lists is Claude's when the move says Claude.
		expect(handoffEndText(view, "claude", LOADED_MODEL)).toMatch(/^Claude /);
		expect(handoffEndText(view, "claude", "opus")).toBe("Claude opus");
		expect(handoffEndText(view, "claude", null)).toBe("Claude");
		expect(handoffEndText(view, "codex", "gpt-5")).toBe("Codex gpt-5");
	});

	it("give the marker's line from the flags alone", () => {
		expect(
			handoffLine(move({ to_local: true, to_model: "qwen3-coder:30b", from_vendor: "codex" }))
		).toBe("Context handoff, Codex to Local · qwen3-coder:30b");
		expect(handoffLine(move({ from_local: true, to_vendor: "codex", to_model: "gpt-5" }))).toBe(
			"Context handoff, Local to Codex gpt-5"
		);
		expect(handoffLine(move({ to_model: LOADED_MODEL }))).toBe(
			`Context handoff, Claude to Claude ${LOADED_MODEL}`
		);
	});
});
