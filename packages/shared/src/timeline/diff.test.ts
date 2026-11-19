// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { FileEdit } from "../bindings";
import {
	changeTotals,
	countsLabel,
	editPath,
	patchChunks,
	patchFiles,
	type PatchLine,
	patchLines,
	patchRuns,
	patchSides,
	shownCounts,
	shownPatch,
} from "./diff";

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

const line = (type: PatchLine["type"], text: string = type): PatchLine => ({
	type,
	number: null,
	text,
});

describe("patchChunks", () => {
	it("starts a chunk at each hunk header", () => {
		const lines = patchLines("@@ -1 +1 @@\n-a\n+b\n@@ -9 +9 @@\n c");
		expect(patchChunks(lines).map((chunk) => chunk.map((l) => l.type))).toEqual([
			["hunk", "remove", "add"],
			["hunk", "context"],
		]);
	});

	it("cuts a long hunk every so many lines", () => {
		const lines = patchLines(Array.from({ length: 5 }, (_, i) => `+${i}`).join("\n"));
		expect(patchChunks(lines, 2).map((chunk) => chunk.length)).toEqual([2, 2, 1]);
	});

	it("gives no chunks for no lines", () => {
		expect(patchChunks([])).toEqual([]);
	});
});

describe("shownPatch", () => {
	const lines = (count: number) =>
		Array.from({ length: count }, (_, i) => `+line ${i}`).join("\n");
	const counts = (shown: ReturnType<typeof shownPatch>) =>
		shown.files.map((file) => [
			file.change.path,
			file.chunks?.reduce((sum, chunk) => sum + chunk.length, 0) ?? null,
		]);

	it("shows every file when they fit", () => {
		const shown = shownPatch(patchFiles([edit("a", lines(3)), edit("b", null)]), 10);
		expect(shown).toMatchObject({ more: false, hidden: 0 });
		expect(counts(shown)).toEqual([
			["a", 3],
			["b", null],
		]);
	});

	it("cuts at the limit and leaves out the files past it", () => {
		const shown = shownPatch(
			patchFiles([edit("a", lines(3)), edit("b", lines(5)), edit("c", lines(2))]),
			6
		);
		expect(shown).toMatchObject({ more: true, hidden: 4 });
		expect(counts(shown)).toEqual([
			["a", 3],
			["b", 3],
		]);
	});

	it("says more is left when only a file without a diff is past the limit", () => {
		const shown = shownPatch(patchFiles([edit("a", lines(4)), edit("b", null)]), 4);
		expect(shown).toMatchObject({ more: true, hidden: 0, files: [{ change: { path: "a" } }] });
	});

	it("keeps each chunk shown whole as it was", () => {
		const files = patchFiles([edit("a", "@@ -1 +1 @@\n-a\n+b\n@@ -9 +9 @@\n c\n+d")]);
		const shown = shownPatch(files, 4);
		expect(shown.files[0]?.chunks?.[0]).toBe(files[0]?.chunks?.[0]);
		expect(shown.files[0]?.chunks?.[1]).toHaveLength(1);
		expect(shown.hidden).toBe(2);
	});

	it("shows everything without a limit", () => {
		const files = patchFiles([edit("a", lines(300))]);
		const shown = shownPatch(files, null);
		expect(shown).toMatchObject({ more: false, hidden: 0 });
		expect(shown.files).toBe(files);
		expect(counts(shown)).toEqual([["a", 300]]);
	});
});

describe("patchSides", () => {
	it("splits a chunk into the code of each side", () => {
		const sides = patchSides([
			line("hunk", "@@ -1,3 +1,3 @@"),
			line("context", "keep"),
			line("remove", "old"),
			line("add", "new"),
			line("add", "more"),
			line("context", "end"),
		]);
		expect(sides.old).toBe("keep\nold\nend");
		expect(sides.new).toBe("keep\nnew\nmore\nend");
		expect(sides.spots).toEqual([
			null,
			{ side: "new", line: 0 },
			{ side: "old", line: 1 },
			{ side: "new", line: 1 },
			{ side: "new", line: 2 },
			{ side: "new", line: 3 },
		]);
	});

	it("gives null for a side with no lines", () => {
		expect(patchSides([line("add", "a")])).toMatchObject({ old: null, new: "a" });
		expect(patchSides([line("remove", "a")])).toMatchObject({ old: "a", new: null });
		expect(patchSides([line("hunk")])).toMatchObject({ old: null, new: null });
	});

	it("keeps a carriage return from splitting a line", () => {
		expect(patchSides([line("add", "a\r"), line("add", "b\rc")]).new).toBe("a\nbc");
	});
});

describe("patchRuns", () => {
	it("groups lines of one kind, and cuts long runs", () => {
		const lines = ["hunk", "context", "add", "add", "add", "remove", "context"].map((type) =>
			line(type as PatchLine["type"])
		);
		expect(patchRuns(lines, 2)).toEqual([
			{ type: "hunk", start: 0, end: 1 },
			{ type: "context", start: 1, end: 2 },
			{ type: "add", start: 2, end: 4 },
			{ type: "add", start: 4, end: 5 },
			{ type: "remove", start: 5, end: 6 },
			{ type: "context", start: 6, end: 7 },
		]);
	});

	it("gives no runs for no lines", () => {
		expect(patchRuns([], 10)).toEqual([]);
	});
});
