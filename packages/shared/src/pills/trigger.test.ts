// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { OBJECT_MARK } from "./draft";
import { detectTrigger } from "./trigger";

/** The trigger with the caret where `|` is. */
const at = (marked: string) => {
	const cursor = marked.indexOf("|");
	return detectTrigger(marked.replace("|", ""), cursor);
};

describe("detectTrigger", () => {
	it("opens the file menu for a word that starts with @", () => {
		expect(at("look at @src/ma|")).toEqual({
			kind: "path",
			query: "src/ma",
			rangeStart: 8,
			rangeEnd: 15,
		});
		expect(at("@|")).toEqual({ kind: "path", query: "", rangeStart: 0, rangeEnd: 1 });
	});

	it("opens the pull request menu for # and a word", () => {
		expect(at("see #41|")).toEqual({
			kind: "git-item",
			query: "41",
			rangeStart: 4,
			rangeEnd: 7,
		});
		expect(at("#cache|")).toEqual({
			kind: "git-item",
			query: "cache",
			rangeStart: 0,
			rangeEnd: 6,
		});
		expect(at("#|")).toEqual({ kind: "git-item", query: "", rangeStart: 0, rangeEnd: 1 });
	});

	it("covers the whole word with the caret inside it", () => {
		expect(at("see #4|12 now")).toEqual({
			kind: "git-item",
			query: "412",
			rangeStart: 4,
			rangeEnd: 8,
		});
	});

	it("opens only at the start of a word", () => {
		expect(at("mail me@home|")).toBeNull();
		expect(at("a#12|")).toBeNull();
		expect(at("|@src")).toBeNull();
		expect(at(`${OBJECT_MARK}@src|`)).toBeNull();
		expect(at("/help|")).toBeNull();
	});

	it("opens the skills menu for any currency sign, but not for money", () => {
		expect(at("use $git|")).toEqual({
			kind: "skill",
			query: "git",
			rangeStart: 4,
			rangeEnd: 8,
		});
		expect(at("$|")).toEqual({ kind: "skill", query: "", rangeStart: 0, rangeEnd: 1 });
		expect(at("€unsl|")?.kind).toBe("skill");
		expect(at("costs $5|")).toBeNull();
		expect(at("$$|")).toBeNull();
		expect(at("so $x^2$|")).toBeNull();
		expect(at("so $x|^2$")).toBeNull();
		expect(at("about $10k.|")).toBeNull();
		expect(at("€9.99|")).toBeNull();
	});

	it("stays shut in code", () => {
		expect(at("run `git show #4|`")).toBeNull();
		expect(at("```\n@src|\n```")).toBeNull();
		expect(at("echo `$HOME|`")).toBeNull();
	});
});
