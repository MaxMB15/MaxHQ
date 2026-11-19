// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { timelineItem } from "../timeline/build";

import { errorsQuote, tellQuote, type VisualOrigin } from "./origin";

const HTML = "<p>Hi</p><script>x()</script>";

const reply = (text: string): VisualOrigin => ({
	agent: { id: "a1", title: "Fix the chart" },
	item: timelineItem({ kind: "assistant_message", text, streaming: false }, { id: "i1" }),
});

describe("errorsQuote", () => {
	it("names the visual and puts each error on its own line", () => {
		expect(errorsQuote("Sales", [{ message: "x is not defined", line: 4 }], 1)).toBe(
			'The visual "Sales" showed an error:\nLine 4: x is not defined'
		);
	});

	it("counts the errors it leaves out", () => {
		expect(
			errorsQuote(
				"",
				[
					{ message: "x is not defined", line: 4 },
					{ message: "y is not defined", line: null },
				],
				5
			)
		).toBe(
			"The visual showed 5 errors:\nLine 4: x is not defined\ny is not defined\nand 3 more"
		);
	});
});

describe("tellQuote", () => {
	it("quotes the errors from the reply, where the visual is in it", () => {
		const origin = reply(`Here it is:\n\n${HTML}`);
		expect(tellQuote(origin, HTML, "Sales", [{ message: "Oops", line: null }], 1)).toEqual({
			type: "quote",
			agent: "a1",
			agent_title: "Fix the chart",
			item: "i1",
			text: 'The visual "Sales" showed an error:\nOops',
			position: 13,
			source: "reply",
			from: null,
		});
	});

	it("starts at the top when the reply doesn't hold the page", () => {
		const quote = tellQuote(reply("No page here"), HTML, "", [{ message: "Oops", line: 1 }], 1);
		expect(quote.position).toBe(0);
	});
});
