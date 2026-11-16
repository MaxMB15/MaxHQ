// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { agentScreenId, fileScreenId, noteScreenId, pullScreenId } from "./navigation";

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

describe("fileScreenId", () => {
	it("gives each of an agent's files its own screen, and keeps it for another line", () => {
		expect(fileScreenId({ params: { agent: "a1", path: "src/a.ts" } })).not.toBe(
			fileScreenId({ params: { agent: "a1", path: "src/b.ts" } })
		);
		expect(fileScreenId({ params: { agent: "a1", path: "src/a.ts" } })).not.toBe(
			fileScreenId({ params: { agent: "a2", path: "src/a.ts" } })
		);
		expect(fileScreenId({ params: { agent: "a1", path: "src/a.ts", line: 12 } })).toBe(
			fileScreenId({ params: { agent: "a1", path: "src/a.ts" } })
		);
	});
});

describe("pullScreenId", () => {
	it("gives each PR its own screen, the same for a repository in any case", () => {
		const pull = { device: "d1", repo: "owner/repo", number: 7 };
		expect(pullScreenId({ params: pull })).not.toBe(
			pullScreenId({ params: { ...pull, number: 8 } })
		);
		expect(pullScreenId({ params: pull })).not.toBe(
			pullScreenId({ params: { ...pull, device: "d2" } })
		);
		expect(pullScreenId({ params: pull })).toBe(
			pullScreenId({ params: { ...pull, repo: "Owner/Repo" } })
		);
	});
});
