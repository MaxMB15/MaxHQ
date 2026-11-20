// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { PullFile } from "../bindings";

import { checksLabel, PULL_STATE, pullFileEdit, pullStateKey, reviewLabel } from "./look";

describe("pullStateKey", () => {
	it("shows GitHub's state, with an open draft apart", () => {
		expect(pullStateKey({ state: "open", draft: false })).toBe("open");
		expect(pullStateKey({ state: "open", draft: true })).toBe("draft");
		expect(pullStateKey({ state: "closed", draft: true })).toBe("closed");
		expect(PULL_STATE[pullStateKey({ state: "merged", draft: false })]).toEqual({
			label: "Merged",
			tone: "info",
		});
	});
});

describe("the words for checks and reviews", () => {
	it("say how each stands", () => {
		expect(checksLabel("failing")).toBe("Checks failing");
		expect(checksLabel("none")).toBe("No checks");
		expect(reviewLabel("changes_requested")).toBe("Changes requested");
	});
});

describe("pullFileEdit", () => {
	const file = (status: PullFile["status"], patch: string | null = "@@ -1 +1 @@\n-a\n+b") => ({
		path: "src/a.ts",
		status,
		additions: 1,
		deletions: 1,
		patch,
	});

	it("reads a changed file as an edit, with its patch as GitHub sent it", () => {
		expect(pullFileEdit(file("modified"))).toEqual({
			path: "src/a.ts",
			old_path: null,
			op: "modify",
			additions: 1,
			deletions: 1,
			patch: "@@ -1 +1 @@\n-a\n+b",
		});
		expect(pullFileEdit(file("added")).op).toBe("add");
		expect(pullFileEdit(file("removed")).op).toBe("delete");
		expect(pullFileEdit(file("renamed")).op).toBe("rename");
		expect(pullFileEdit(file("changed", null)).patch).toBeNull();
	});
});
