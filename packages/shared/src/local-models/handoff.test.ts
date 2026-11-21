// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { LOADED_MODEL } from "../data/mock-local-models";
import { mockSource } from "../data/mock-source";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { handoffEndText } from "./handoff";

describe("a move's ends", () => {
	it("name a local model's end Local, never Claude", async () => {
		const view = applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());
		expect(handoffEndText(view, "claude", LOADED_MODEL)).toBe("Local · Qwen3.6-35B-A3B");
		expect(handoffEndText(view, "claude", "qwen3-coder:30b")).toBe("Local · qwen3-coder:30b");
		expect(handoffEndText(view, "claude", "opus")).toBe("Claude opus");
		expect(handoffEndText(view, "claude", null)).toBe("Claude");
		expect(handoffEndText(view, "codex", "gpt-5")).toBe("Codex gpt-5");
	});
});
