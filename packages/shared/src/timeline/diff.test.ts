// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { FileEdit } from "../bindings";
import { changeTotals, countsLabel, editPath, patchLines, shownCounts, shownPatch } from "./diff";

const edit = (path: string, patch: string | null, additions = 0, deletions = 0): FileEdit => ({
	path,
	old_path: null,
	op: "modify",
	additions,
	deletions,
	patch,
});

describe("patchLines", () => {
	it("reads hunks with their line numbers and leaves out the file headers", () => {
		const patch = [
			"diff --git a/src/a.ts b/src/a.ts",
			"--- a/src/a.ts",
			"+++ b/src/a.ts",
			"@@ -3,3 +3,3 @@ fn main",
			" keep",
			"-old",
			"+new",
			"\\ No newline at end of file",
			"@@ -20 +20,2 @@",
			" tail",
			"+more",
			"",
		].join("\n");
		expect(patchLines(patch)).toEqual([
			{ type: "hunk", number: null, text: "@@ -3,3 +3,3 @@ fn main" },
			{ type: "context", number: 3, text: "keep" },
			{ type: "remove", number: 4, text: "old" },
			{ type: "add", number: 4, text: "new" },
			{ type: "hunk", number: null, text: "@@ -20 +20,2 @@" },
			{ type: "context", number: 20, text: "tail" },
			{ type: "add", number: 21, text: "more" },
		]);
	});

	it("skips the headers of a second file", () => {
		const patch = [
			"@@ -1 +1 @@",
			"-a",
			"diff --git a/b.ts b/b.ts",
			"--- a/b.ts",
			"+++ b/b.ts",
			"@@ -1 +1 @@",
			"+b",
		].join("\n");
		expect(patchLines(patch).map((line) => line.type)).toEqual([
			"hunk",
			"remove",
			"hunk",
			"add",
		]);
	});

	it("reads a patch without hunk headers as one hunk without numbers", () => {
		expect(patchLines("-let a = 1;\n+let a = 2;\n+let b = 3;\n")).toEqual([
			{ type: "remove", number: null, text: "let a = 1;" },
			{ type: "add", number: null, text: "let a = 2;" },
			{ type: "add", number: null, text: "let b = 3;" },
		]);
	});

	it("reads an empty patch as no lines", () => {
		expect(patchLines("")).toEqual([]);
	});
});

describe("editPath", () => {
	it("names both paths of a rename", () => {
		expect(editPath(edit("src/b.ts", null))).toBe("src/b.ts");
		expect(editPath({ ...edit("src/b.ts", null), op: "rename", old_path: "src/a.ts" })).toBe(
			"src/a.ts → src/b.ts"
		);
	});
});

describe("shownCounts", () => {
	it("shows what a change adds unless it only removes", () => {
		expect(shownCounts(12, 3)).toEqual({ additions: 12, deletions: 3 });
		expect(shownCounts(0, 3)).toEqual({ additions: null, deletions: 3 });
		expect(shownCounts(0, 0)).toEqual({ additions: 0, deletions: null });
	});

	it("reads the same counts out in words", () => {
		expect(countsLabel(12, 3)).toBe("12 added, 3 removed");
		expect(countsLabel(0, 3)).toBe("3 removed");
		expect(countsLabel(0, 0)).toBe("0 added");
	});
});

describe("changeTotals", () => {
	it("adds up every file", () => {
		expect(changeTotals([edit("a", null, 2, 1), edit("b", null, 10, 0)])).toEqual({
			additions: 12,
			deletions: 1,
		});
	});
});

describe("shownPatch", () => {
	const lines = (count: number) =>
		Array.from({ length: count }, (_, i) => `+line ${i}`).join("\n");

	it("shows every file when they fit", () => {
		const shown = shownPatch([edit("a", lines(3)), edit("b", null)], 10);
		expect(shown.more).toBe(false);
		expect(shown.files.map((file) => [file.change.path, file.lines?.length ?? null])).toEqual([
			["a", 3],
			["b", null],
		]);
	});

	it("cuts at the limit and leaves out the files past it", () => {
		const shown = shownPatch(
			[edit("a", lines(3)), edit("b", lines(5)), edit("c", lines(2))],
			6
		);
		expect(shown.more).toBe(true);
		expect(shown.files.map((file) => [file.change.path, file.lines?.length])).toEqual([
			["a", 3],
			["b", 3],
		]);
	});

	it("says more is left when only a file without a diff is past the limit", () => {
		const shown = shownPatch([edit("a", lines(4)), edit("b", null)], 4);
		expect(shown).toMatchObject({ more: true, files: [{ change: { path: "a" } }] });
	});

	it("shows everything without a limit", () => {
		const shown = shownPatch([edit("a", lines(300))], null);
		expect(shown.more).toBe(false);
		expect(shown.files[0]?.lines).toHaveLength(300);
	});
});
