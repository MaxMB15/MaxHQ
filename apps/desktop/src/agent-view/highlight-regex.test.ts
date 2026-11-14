// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// V8 doesn't have the WebKit bug the rewrite works around, so these tests
// check that the rewrite is right and that it changes nothing V8 can see.

import { defaultJavaScriptRegexConstructor } from "shiki/engine/javascript";
import { describe, expect, it } from "vitest";

import { regexFor, unanchor } from "./highlight-regex";
import { GRAMMARS } from "./highlight-worker";

describe("unanchor", () => {
	it("keeps a leading ^", () => {
		expect(unanchor("^a", "dgv")).toBe("^a");
	});

	it("rewrites a ^ inside a group", () => {
		expect(unanchor("(^[\\t ]+)?(?=\\/\\/)", "dgv")).toBe("((?<![^])[\\t ]+)?(?=\\/\\/)");
		expect(unanchor("a|^b", "dg")).toBe("a|(?<![^])b");
	});

	it("leaves negated classes and escaped carets alone", () => {
		expect(unanchor("a[^b]\\^", "dg")).toBe("a[^b]\\^");
		expect(unanchor("a[[^b]&&[^c]]", "dgv")).toBe("a[[^b]&&[^c]]");
	});

	it("leaves multiline patterns alone", () => {
		expect(unanchor("a(^b)", "dgm")).toBe("a(^b)");
	});
});

const SAMPLES = [
	'const f = (x) => n // done x || "y";',
	"\t  # note: it's fine /* here */ -- and ; there",
	"fn main() { let v = vec![1, 2]; } // end\n  -- x",
];

const patterns = async (): Promise<string[]> => {
	const found = new Set<string>();
	const walk = (node: unknown) => {
		if (!node || typeof node !== "object") return;
		for (const key of ["match", "begin", "end", "while"]) {
			const value = (node as Record<string, unknown>)[key];
			if (typeof value === "string") found.add(value);
		}
		for (const child of Object.values(node)) walk(child);
	};
	for (const load of Object.values(GRAMMARS)) walk((await load()).default);
	return [...found];
};

const compile = (make: () => RegExp): RegExp | null => {
	try {
		return make();
	} catch {
		return null;
	}
};

const run = (re: RegExp, text: string, at: number) => {
	re.lastIndex = at;
	const m = re.exec(text);
	return m && { index: m.index, groups: [...m], indices: m.indices };
};

describe("regexFor", () => {
	it("matches like Shiki's own regexes in every grammar the worker loads", async () => {
		const all = await patterns();
		expect(all.length).toBeGreaterThan(1000);
		let rewritten = 0;
		for (const pattern of all) {
			const plain = compile(() =>
				defaultJavaScriptRegexConstructor(pattern, { target: "auto" })
			);
			const ours = compile(() => regexFor(pattern));
			expect(ours === null, pattern).toBe(plain === null);
			if (!plain || !ours) continue;
			if (ours.source !== plain.source) rewritten++;
			for (const text of SAMPLES) {
				for (let at = 0; at <= text.length; at++) {
					expect(run(ours, text, at), pattern).toEqual(run(plain, text, at));
				}
			}
		}
		expect(rewritten).toBeGreaterThan(0);
	}, 60_000);
});
