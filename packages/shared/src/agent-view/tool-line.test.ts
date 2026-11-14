// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ItemBody, ItemStatus } from "../bindings";
import { timelineItem } from "../timeline/build";
import { inlineCode, TOOL_VERBS, toolLines, verbFor } from "./tool-line";

const names = {
	agent: (id: string) => (id === "a2" ? "Fix the login" : undefined),
	device: () => "devbox",
};

const lines = (body: ItemBody, status: ItemStatus = "completed") =>
	toolLines(timelineItem(body, { id: "i", status }), names);

describe("verbFor", () => {
	it("picks the verb by status", () => {
		expect(verbFor("running", TOOL_VERBS.command)).toBe("Running");
		expect(verbFor("waiting", TOOL_VERBS.command)).toBe("Wants to run");
		expect(verbFor("declined", TOOL_VERBS.command)).toBe("Didn't run");
		expect(verbFor("completed", TOOL_VERBS.command)).toBe("Ran");
		expect(verbFor("waiting", TOOL_VERBS.read)).toBe("Reading");
		expect(verbFor("declined", TOOL_VERBS.read)).toBe("Read");
	});
});

describe("inlineCode", () => {
	it("fences past the backticks inside", () => {
		expect(inlineCode("ls")).toBe("`ls`");
		expect(inlineCode("echo `date`")).toBe("`` echo `date` ``");
		expect(inlineCode("a ``b`` c")).toBe("```a ``b`` c```");
	});

	it("keeps it on one line", () => {
		expect(inlineCode("  a\n  b ")).toBe("`a b`");
	});
});

describe("toolLines", () => {
	it("writes each kind of step as a line", () => {
		const command = {
			kind: "command",
			command: "pnpm test\n--watch",
			cwd: null,
			exit_code: 0,
			output: "",
			output_truncated: false,
			background: false,
		} as const;
		expect(lines(command)).toEqual(["Ran `pnpm test`"]);
		expect(lines(command, "failed")).toEqual(["Ran `pnpm test` (failed)"]);
		expect(lines(command, "declined")).toEqual(["Didn't run `pnpm test` (declined)"]);
		expect(lines({ kind: "file_read", path: "src/a.ts", lines: null })).toEqual([
			"Read `src/a.ts`",
		]);
		expect(lines({ kind: "search", query: "login", scope: "src", matches: 3 })).toEqual([
			"Searched for `login` in `src`",
		]);
		expect(lines({ kind: "web", action: "search", target: "tauri dialog" })).toEqual([
			"Searched the web for `tauri dialog`",
		]);
		expect(lines({ kind: "web", action: "fetch", target: "https://example.com" })).toEqual([
			"Fetched https://example.com",
		]);
		expect(
			lines({
				kind: "tool_call",
				server: "github",
				name: "get_issue",
				title: "Issue 12",
				input: null,
				output: null,
			})
		).toEqual(["Used `github:get_issue`, Issue 12"]);
	});

	it("gives a line for each file a change touched", () => {
		const edit = { patch: null, additions: 0, deletions: 0, old_path: null };
		expect(
			lines({
				kind: "file_change",
				changes: [
					{ ...edit, op: "modify", path: "a.ts", additions: 3, deletions: 1 },
					{ ...edit, op: "add", path: "b.ts", additions: 9 },
					{ ...edit, op: "rename", path: "d.ts", old_path: "c.ts" },
				],
			})
		).toEqual(["Edited `a.ts` (+3 −1)", "Created `b.ts`", "Renamed `c.ts` to `d.ts`"]);
	});

	it("words an agent tool the way its row does", () => {
		expect(
			lines({
				kind: "tool_call",
				server: "maxhq",
				name: "read_agent",
				title: null,
				input: JSON.stringify({ agent: "a2" }),
				output: null,
			})
		).toEqual(["Read Fix the login"]);
	});

	it("has nothing for other items", () => {
		expect(lines({ kind: "assistant_message", text: "Hi", streaming: false })).toEqual([]);
	});
});
