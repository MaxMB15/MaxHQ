// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { findQuote, type QuoteAnchor, quoteAnchor } from "./anchor";

const anchor = (text: string, position = 0): QuoteAnchor => ({ text, position });

/** The text a span covers. */
const at = (text: string, span: { from: number; to: number } | null) =>
	span === null ? null : text.slice(span.from, span.to);

describe("quoteAnchor", () => {
	it("keeps where it starts with whitespace runs as one space", () => {
		const text = "Before \n  quote\t \n after";
		expect(quoteAnchor(text, "Before \n  ".length, "Before \n  quote".length)).toEqual({
			text: "quote",
			position: "Before ".length,
		});
	});

	it("trims the whitespace a selection picks up at its ends", () => {
		const text = "One line.\nThe next line.\n";
		expect(quoteAnchor(text, "One line.".length, text.length)).toEqual({
			text: "The next line.",
			position: "One line. ".length,
		});
	});

	it("keeps the line breaks inside a quote", () => {
		const text = "Run this:\nif (ready) {\n  go();\n}\nDone.";
		const quote = quoteAnchor(text, text.indexOf("if"), text.indexOf("}") + 1);
		expect(quote?.text).toBe("if (ready) {\n  go();\n}");
		expect(quote?.position).toBe("Run this: ".length);
	});

	it("takes the offsets in either order", () => {
		expect(quoteAnchor("left quote right", 10, 5)).toEqual({ text: "quote", position: 5 });
	});

	it("gives nothing for whitespace alone", () => {
		expect(quoteAnchor("a \n b", 1, 4)).toBeNull();
		expect(quoteAnchor("abc", 2, 2)).toBeNull();
	});

	it("counts in UTF-16 units", () => {
		const text = "😀 café quote";
		expect(quoteAnchor(text, text.indexOf("quote"), text.length)?.position).toBe(8);
	});
});

describe("findQuote", () => {
	it("goes back to where a quote was made", () => {
		const text = "Run this:\nif (ready) {\n  go();\n}\nDone. And again: if (ready) {";
		const from = text.indexOf("if");
		const made = quoteAnchor(text, from, text.indexOf("}") + 1)!;
		expect(findQuote(text, made)).toEqual({ from, to: text.indexOf("}") + 1 });
	});

	it("tells repeats apart by where each starts", () => {
		const text = "quote / quote / quote";
		expect(findQuote(text, anchor("quote", 0))).toEqual({ from: 0, to: 5 });
		expect(findQuote(text, anchor("quote", 8))).toEqual({ from: 8, to: 13 });
		expect(findQuote(text, anchor("quote", 16))).toEqual({ from: 16, to: 21 });
	});

	it("finds places that overlap", () => {
		expect(findQuote("banana", anchor("ana", 3))).toEqual({ from: 3, to: 6 });
	});

	it("still holds while the row grows at its end", () => {
		const made = quoteAnchor("Look at the quote", 12, 17)!;
		expect(findQuote("Look at the quote, and then a quote more", made)).toEqual({
			from: 12,
			to: 17,
		});
	});

	it("takes the place nearest where it was when the text before it changed", () => {
		const text = "A new line. The quote, and later the quote again.";
		expect(findQuote(text, anchor("quote", 4))).toEqual({ from: 16, to: 21 });
		expect(findQuote(text, anchor("quote", 40))).toEqual({ from: 37, to: 42 });
	});

	it("gives nothing when the quote is gone", () => {
		expect(findQuote("Nothing like it here.", anchor("quoted words"))).toBeNull();
		expect(findQuote("Anything", anchor("  "))).toBeNull();
	});

	it("matches code after its indentation and line endings change", () => {
		const text = "Example:\nif (ready) {\n\trun();\n}\nDone.";
		const span = findQuote(text, anchor("if (ready) {\r\n    run();\r\n}", 9));
		expect(span).toEqual({ from: 9, to: 31 });
	});

	it("counts no-break spaces as whitespace", () => {
		const text = "before \n quoted\t text \nafter";
		expect(at(text, findQuote(text, anchor("quoted  text")))).toBe("quoted\t text");
	});

	it("counts in UTF-16 units around emoji and combining marks", () => {
		expect(findQuote("😀 cafe\u0301 🚀 done", anchor("cafe\u0301 🚀", 3))).toEqual({
			from: 3,
			to: 11,
		});
	});
});
