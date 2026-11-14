// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { timelineItem } from "../timeline/build";

import { makeQuote, parseQuote, quoteSource } from "./quote";

const command = (line: string) =>
	timelineItem(
		{
			kind: "command",
			command: line,
			cwd: null,
			exit_code: 0,
			output: "ok",
			output_truncated: false,
			background: false,
		},
		{ id: "i1" }
	);

describe("quoteSource", () => {
	it("tells a reply, your own message and a question apart", () => {
		expect(quoteSource({ kind: "assistant_message", text: "Hi", streaming: false })).toEqual({
			source: "reply",
			from: null,
		});
		expect(
			quoteSource({ kind: "user_message", text: "Go", attachments: [], delivery: "sent" })
				.source
		).toBe("request");
		expect(
			quoteSource({ kind: "question", attention: "at1", questions: [], answers: null }).source
		).toBe("question");
	});

	it("names the command that printed an output by its first line", () => {
		expect(quoteSource(command("  cargo test\n--workspace"))).toEqual({
			source: "output",
			from: "cargo test",
		});
		const long = quoteSource(command(`echo ${"x".repeat(200)}`)).from!;
		expect(long).toHaveLength(80);
		expect(long.endsWith("…")).toBe(true);
	});

	it("names a tool by its name and a sub-agent by its kind", () => {
		expect(
			quoteSource({
				kind: "tool_call",
				server: "docs",
				name: "search_docs",
				title: null,
				input: null,
				output: "found",
			})
		).toEqual({ source: "output", from: "search_docs" });
		expect(
			quoteSource({
				kind: "subagent",
				subagent: null,
				agent_type: null,
				description: "Look around",
				result: "Done",
			})
		).toEqual({ source: "output", from: null });
	});

	it("calls anything else other", () => {
		expect(quoteSource({ kind: "plan", markdown: "1. Go", streaming: false }).source).toBe(
			"other"
		);
		expect(quoteSource({ kind: "error", message: "Boom", retrying: false }).source).toBe(
			"other"
		);
	});
});

describe("makeQuote", () => {
	it("names the agent, the row and the passage", () => {
		const quote = makeQuote({ id: "a1", title: "Fix the build" }, command("cargo test"), {
			text: "ok",
			position: 0,
		});
		expect(quote).toEqual({
			type: "quote",
			agent: "a1",
			agent_title: "Fix the build",
			item: "i1",
			text: "ok",
			position: 0,
			source: "output",
			from: "cargo test",
		});
		expect(parseQuote(JSON.stringify(quote))).toEqual(quote);
	});
});

describe("parseQuote", () => {
	it("refuses anything that isn't a quote", () => {
		expect(parseQuote(null)).toBeNull();
		expect(parseQuote("")).toBeNull();
		expect(parseQuote("not json")).toBeNull();
		expect(parseQuote(JSON.stringify({ type: "attachment", key: "k" }))).toBeNull();
		expect(
			parseQuote(
				JSON.stringify({
					type: "quote",
					agent: "a1",
					agent_title: "A",
					item: "i1",
					text: "t",
					position: 0,
					source: "elsewhere",
					from: null,
				})
			)
		).toBeNull();
	});
});
