// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { createHighlighterCoreSync } from "shiki/core";
import python from "shiki/langs/python.mjs";
import typescript from "shiki/langs/typescript.mjs";
import theme from "shiki/themes/github-dark-default.mjs";
import { describe, expect, it } from "vitest";

import { codeColor } from "../theme";
import {
	engineFor,
	grammarFor,
	highlight,
	nextStep,
	tokenizeSliced,
	tokenStyle,
} from "./highlight";

const shikiWith = (hasIndices: boolean) =>
	createHighlighterCoreSync({
		themes: [theme],
		langs: [typescript, python],
		engine: engineFor(hasIndices),
	});

const now = async () => {};

const CODE = [
	"/**",
	" * A comment over",
	" * several lines.",
	" */",
	"export const greet = (name: string): string => {",
	"\tconst text = `hello ${name}",
	"and more`;",
	"\treturn text; // done",
	"};",
	"",
].join("\n");

describe("tokenizeSliced", () => {
	const shiki = shikiWith(true);

	it("gives the same lines in slices as in one go", async () => {
		const whole = await tokenizeSliced(shiki, CODE, "typescript", {
			pause: now,
			step: () => 500,
		});
		for (const size of [1, 2, 3]) {
			const sliced = await tokenizeSliced(shiki, CODE, "typescript", {
				pause: now,
				step: () => size,
			});
			expect(sliced).toEqual(whole);
		}
	});

	it("gives one line for each line of the code", async () => {
		const lines = await tokenizeSliced(shiki, CODE, "typescript", { pause: now });
		expect(lines).toHaveLength(CODE.split("\n").length);
		expect(lines.map((line) => line.map((t) => t.content).join(""))).toEqual(CODE.split("\n"));
	});

	it("takes the app's code colours", async () => {
		const lines = await tokenizeSliced(shiki, "# note\nx = 'a'", "python", {
			pause: now,
			colors: { "#8b949e": "#010203" },
		});
		expect(lines[0]!.every((t) => t.color === "#010203")).toBe(true);
	});

	it("pauses between slices and not after the last", async () => {
		let pauses = 0;
		await tokenizeSliced(shiki, CODE, "typescript", {
			pause: async () => {
				pauses += 1;
			},
			step: () => 4,
		});
		expect(pauses).toBe(2);
	});
});

describe("engineFor", () => {
	it("colours code the same with guessed group places", async () => {
		const options = { pause: now, step: () => 500 };
		const sample = `${CODE}\ndef f(x: int) -> str:\n\treturn f"{x!r}"  # done\n`;
		for (const lang of ["typescript", "python"]) {
			expect(await tokenizeSliced(shikiWith(false), sample, lang, options)).toEqual(
				await tokenizeSliced(shikiWith(true), sample, lang, options)
			);
		}
	});
});

describe("nextStep", () => {
	it("takes fewer lines after a slow slice and more after a quick one", () => {
		expect(nextStep(20, 24)).toBe(10);
		expect(nextStep(20, 6)).toBe(40);
	});

	it("keeps between one and 500 lines", () => {
		expect(nextStep(1, 1_000)).toBe(1);
		expect(nextStep(400, 0)).toBe(500);
	});
});

describe("grammarFor", () => {
	it("draws JavaScript with TypeScript's grammar", () => {
		expect(grammarFor("javascript")).toBe("typescript");
		expect(grammarFor("jsx")).toBe("tsx");
	});

	it("keeps its own grammar for a language the phone carries", () => {
		expect(grammarFor("rust")).toBe("rust");
	});

	it("leaves the languages the phone doesn't carry plain", () => {
		expect(grammarFor("html")).toBeNull();
		expect(grammarFor("java")).toBeNull();
	});
});

describe("highlight", () => {
	it("loads Shiki and a grammar when code first asks", async () => {
		const lines = await highlight("// hi\nlet a = 1;", "javascript");
		expect(lines?.[0]?.[0]).toMatchObject({ content: "// hi", color: codeColor.comment });
		expect(lines?.[1]?.some((t) => t.color === codeColor.keyword)).toBe(true);
	});

	it("leaves a language without a grammar plain", async () => {
		expect(await highlight("<p>hi</p>", "html")).toBeNull();
	});
});

describe("tokenStyle", () => {
	it("gives one style for each look", () => {
		const a = tokenStyle({ content: "a", color: "#ffffff", fontStyle: 1 });
		expect(tokenStyle({ content: "b", color: "#ffffff", fontStyle: 1 })).toBe(a);
		expect(a).toEqual({ color: "#ffffff", fontStyle: "italic", fontWeight: undefined });
		expect(tokenStyle({ content: "c", fontStyle: 2 }).fontWeight).toBe("600");
	});
});
