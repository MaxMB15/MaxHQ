// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { DeviceReach } from "@maxhq/shared/status/reach";
import { agent, entities, link, workspace } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { agentSummary, reachSummary } from "./summary";

const reach = (look: DeviceReach["look"]): DeviceReach => ({
	id: look,
	name: look,
	look,
	title: "",
	detail: "",
	reached: look === "live",
	via: null,
	routes: [],
	pairAgain: false,
	version: null,
});

describe("reachSummary", () => {
	it("counts the live, the connecting and the rest", () => {
		expect(
			reachSummary(
				["live", "live", "connecting", "unreachable", "agent_down"].map((look) =>
					reach(look as DeviceReach["look"])
				)
			)
		).toBe("2 live, 1 connecting, 2 not working");
		expect(reachSummary([reach("live")])).toBe("1 live");
	});
});

describe("agentSummary", () => {
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			links: [
				link({ id: "box", device: "dev-box" }),
				link({ id: "empty", device: "dev-empty" }),
			],
			workspaces: [workspace({ id: "w1", device: "dev-box" })],
			agents: [
				agent({ id: "a", workspace: "w1" }),
				agent({
					id: "b",
					workspace: "w1",
					activity: { state: "needs_input", attention: "e" },
				}),
				agent({ id: "sub", workspace: "w1", parent: "a" }),
			],
		}),
	});

	it("counts a device's agents and those waiting on you, without sub-agents", () => {
		expect(agentSummary(view, "box")).toBe("2 agents, 1 needs you");
	});

	it("says nothing for a device with no agents", () => {
		expect(agentSummary(view, "empty")).toBeNull();
	});
});
