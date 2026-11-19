// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { defaultJavaScriptRegexConstructor } from "shiki/engine/javascript";
import { describe, expect, it } from "vitest";

import { guessIndices, HAS_INDICES, withGuessedIndices } from "./match-indices";

// The places the `d` flag gives, to check the guesses against.
const realIndices = (source: string, input: string, at = 0) => {
	const regex = new RegExp(source, "dg");
	regex.lastIndex = at;
	return regex.exec(input)!.indices!.map((pair) => (pair ? [pair[0], pair[1]] : undefined));
};

const guessed = (source: string, input: string, at = 0) => {
	const regex = new RegExp(source, "g");
	regex.lastIndex = at;
	return guessIndices(regex.exec(input)!);
};

describe("guessIndices", () => {
	it("knows the engine here takes the d flag", () => {
		expect(HAS_INDICES).toBe(true);
	});

	it.each([
		["(a+)(b)?(c)", "xxaacyy"],
		["(\\w+)\\s*(=)\\s*(\\w+)", "let name = value"],
		["(\\/\\/)(.*)", "x = 1 // a // b"],
		["((a)(b))(c)", "zzabc"],
		["(?<=(ab))c", "abc"],
		["(x)|(y)", "y"],
	])("finds the groups of %s in %j", (source, input) => {
		expect(guessed(source, input)).toEqual(realIndices(source, input));
	});

	it("starts from where the match starts", () => {
		expect(guessed("(b)", "b b b", 2)).toEqual(realIndices("(b)", "b b b", 2));
	});

	it("leaves a group that took no part undefined", () => {
		expect(guessed("(a)?(b)", "b")).toEqual([[0, 1], undefined, [0, 1]]);
	});
});

describe("withGuessedIndices", () => {
	it("gives a plain regex's matches indices", () => {
		const regex = withGuessedIndices(/(\w+)(\()/g);
		regex.lastIndex = 4;
		const match = regex.exec("let call(1)")!;
		expect(match.indices).toEqual([
			[4, 9],
			[4, 8],
			[8, 9],
		]);
		expect(regex.lastIndex).toBe(9);
	});

	it("leaves a miss alone", () => {
		expect(withGuessedIndices(/z/g).exec("abc")).toBeNull();
	});

	it("works on the regexes Shiki's engine builds", () => {
		const source = "(?>(\\w+))\\s*(\\.)";
		const plain = withGuessedIndices(
			defaultJavaScriptRegexConstructor(source, { target: "auto", hasIndices: false })
		);
		const real = defaultJavaScriptRegexConstructor(source, {
			target: "auto",
			hasIndices: true,
		});
		plain.lastIndex = 2;
		real.lastIndex = 2;
		expect(plain.exec("x abc .d")!.indices).toEqual(
			[...real.exec("x abc .d")!.indices!].map((pair) => pair && [pair[0], pair[1]])
		);
	});
});
