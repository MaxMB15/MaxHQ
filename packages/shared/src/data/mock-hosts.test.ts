// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { NOW } from "../test/build";
import { applySnapshot, idOf } from "../view/replica";
import { MOCK_MOVE, mockHosts } from "./mock-hosts";
import { DEVBOX, MAC } from "./mock-view";

describe("mockHosts", () => {
	it("keys each entity the way the replica does", () => {
		const view = applySnapshot({ seq: 1, entities: mockHosts(NOW) });
		expect(view.tables.host_report.get(DEVBOX)?.items).toHaveLength(5);
		expect(view.tables.host_report.get(MAC)?.source).toEqual({ type: "defaults" });
		expect(view.tables.move.get(MOCK_MOVE)?.target).toBe("mhq-alpha");
		expect(view.tables.acp_registry.get(MAC)?.agents.map((a) => a.id)).toEqual([
			"gemini",
			"goose",
			"kiro",
			"fast-agent",
			"mac-only",
		]);
		expect(view.tables.acp_registry.get(DEVBOX)?.problem).not.toBeNull();
		expect(mockHosts(NOW).map(idOf)).toEqual([
			DEVBOX,
			MAC,
			MOCK_MOVE,
			MAC,
			DEVBOX,
			"move-bravo",
			"move-charlie",
			"move-delta",
			"move-echo",
		]);
	});

	it("has a move in each state a card shows", () => {
		const moves = mockHosts(NOW).filter((e) => e.entity === "move");
		const states = new Set(moves.map((m) => (m.entity === "move" ? m.state.state : "")));
		expect(states).toEqual(new Set(["ready", "failed", "moved_back"]));
		expect(moves.some((m) => m.entity === "move" && m.source_changed)).toBe(true);
		expect(moves.some((m) => m.entity === "move" && m.kept_until !== null)).toBe(true);
	});
});
