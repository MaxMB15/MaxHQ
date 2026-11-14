// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { FOLD_ABOVE, foldWorkflows, workflowsFolded } from "./fold";

describe("the Workflows fold", () => {
	const none: ReadonlySet<string> = new Set();

	it("starts folded only past a few runs", () => {
		expect(workflowsFolded(none, "p1", FOLD_ABOVE)).toBe(false);
		expect(workflowsFolded(none, "p1", FOLD_ABOVE + 1)).toBe(true);
	});

	it("stays the way you left it as runs come and go", () => {
		const opened = foldWorkflows(none, ["p1"], false);
		expect(workflowsFolded(opened, "p1", 40)).toBe(false);
		const folded = foldWorkflows(opened, ["p1"], true);
		expect(workflowsFolded(folded, "p1", 1)).toBe(true);
		expect(workflowsFolded(folded, "p2", 1)).toBe(false);
	});

	it("folds several projects at once and keeps other folds", () => {
		const start = new Set(["project-key", "notes:p1"]);
		const next = foldWorkflows(start, ["p1", "p2"], true);
		expect(workflowsFolded(next, "p1", 0)).toBe(true);
		expect(workflowsFolded(next, "p2", 0)).toBe(true);
		expect(next.has("project-key") && next.has("notes:p1")).toBe(true);
	});
});
