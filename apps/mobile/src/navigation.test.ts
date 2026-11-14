// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { agentScreenId, noteScreenId } from "./navigation";

describe("agentScreenId", () => {
	it("gives each agent its own screen, and keeps one agent's screen for a new prompt", () => {
		expect(agentScreenId({ params: { agent: "a1" } })).not.toBe(
			agentScreenId({ params: { agent: "a2" } })
		);
		expect(agentScreenId({ params: { agent: "a1", attention: "e1" } })).toBe(
			agentScreenId({ params: { agent: "a1" } })
		);
	});
});

describe("noteScreenId", () => {
	it("gives each note its own screen, and keeps one note's screen when it was just made", () => {
		expect(noteScreenId({ params: { project: "p1", note: "n1" } })).not.toBe(
			noteScreenId({ params: { project: "p1", note: "n2" } })
		);
		expect(noteScreenId({ params: { project: "p1", note: "n1" } })).not.toBe(
			noteScreenId({ params: { project: "p2", note: "n1" } })
		);
		expect(noteScreenId({ params: { project: "p1", note: "n1", made: true } })).toBe(
			noteScreenId({ params: { project: "p1", note: "n1" } })
		);
	});
});
