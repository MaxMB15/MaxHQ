// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { mockSource } from "../data/mock-source";
import { MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { agentScanPlace, feedOf } from "./feed";

describe("the $ menu's feed", () => {
	it("scans an agent's workspace on the device that holds it", async () => {
		const view = applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());
		const agent = [...view.tables.agent.values()].find((a) => a.workspace === "ws-mac-maxhq");
		expect(agentScanPlace(view, agent!.id)).toEqual({
			device: MAC,
			place: { type: "workspace", workspace: "ws-mac-maxhq" },
		});
		expect(agentScanPlace(view, "agent-gone")).toBeNull();
	});

	it("waits for the first scan, keeps the last skills through a failure, and says why there are none", () => {
		expect(feedOf("codex", false, undefined)).toEqual({
			cli: "codex",
			skills: null,
			loading: false,
			notice: "This agent's device doesn't list skills",
		});
		expect(feedOf("codex", true, undefined).loading).toBe(true);
		expect(feedOf("codex", true, { state: "scanning", skills: null }).loading).toBe(true);
		expect(feedOf("codex", true, { state: "failed", error: "x", skills: null })).toMatchObject({
			loading: false,
			notice: "Couldn't read this agent's skills",
		});
		expect(feedOf("codex", true, { state: "failed", error: "x", skills: [] })).toMatchObject({
			skills: [],
			loading: false,
			notice: null,
		});
	});
});
