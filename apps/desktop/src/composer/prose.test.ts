// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { isCodeLike, prose, proseLines } from "./prose";

describe("isCodeLike", () => {
	it("takes flags, mentions, paths, files, symbols and camelCase for code", () => {
		for (const word of [
			"--force",
			"@max",
			"/usr/bin",
			"~/notes",
			"main.rs",
			"snake_case",
			"getUserName",
			"v2",
			"a=b",
			"(src/lib.rs)",
		]) {
			expect(isCodeLike(word), word).toBe(true);
		}
	});

	it("leaves plain words, capitals and punctuation as prose", () => {
		for (const word of ["hello", "Hello", "don't", "end.", "(aside)", "“quoted”", "Tauri,"]) {
			expect(isCodeLike(word), word).toBe(false);
		}
	});
});

describe("proseLines", () => {
	it("blanks inline code, links and paths, keeping every offset", () => {
		const lines = [
			"Fix the `logn` bug",
			"See https://exmple.com/teh and www.teh.dev",
			"Open ~/Code/teh.rs and /usr/lib/wrold",
		];
		const out = proseLines(lines);
		expect(out.map((line) => line.length)).toEqual(lines.map((line) => line.length));
		expect(out[0]).toBe(`Fix the ${" ".repeat(6)} bug`);
		expect(out.map((line) => line.replace(/ +/g, " ").trim())).toEqual([
			"Fix the bug",
			"See and",
			"Open and",
		]);
	});

	it("keeps a link's label as prose and blanks its target", () => {
		expect(prose("See [the lnik](https://teh.example/a) and [notes](./teh.md)")).toBe(
			"See [the lnik]" + " ".repeat(23) + " and [notes]" + " ".repeat(10)
		);
	});

	it("blanks a fenced code block, fences included, until the fence closes", () => {
		const out = proseLines(["Before", "```ts", "const teh = 1;", "```", "After teh"]);
		expect(out).toEqual(["Before", "     ", " ".repeat(14), "   ", "After teh"]);
	});

	it("reads only the fences outside the lines asked for", () => {
		const out = proseLines(["teh", "```", "teh", "```", "teh", "wrold"], 2, 5);
		expect(out).toEqual(["", "", "   ", "   ", "teh", ""]);
	});

	it("leaves the lines after a one-line code span as prose", () => {
		const out = proseLines(["```a``` b", "After teh"]);
		expect(out[1]).toBe("After teh");
	});

	it("needs the same fence to close a block", () => {
		const out = proseLines(["~~~", "```", "still code", "~~~", "prose"]);
		expect(out.slice(1, 4).every((line) => line.trim() === "")).toBe(true);
		expect(out[4]).toBe("prose");
	});

	it("blanks only the word an unclosed backtick opens", () => {
		expect(prose("run `x and go")).toBe("run    and go");
	});

	it("matches a double backtick span that holds a single one", () => {
		expect(prose("a ``x ` y`` b")).toBe("a " + " ".repeat(9) + " b");
	});
});
