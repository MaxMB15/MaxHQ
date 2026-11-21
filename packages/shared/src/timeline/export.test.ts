// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AttachmentRef, ItemBody, TimelineItem } from "../bindings";
import { OBJECT_MARK } from "../pills/draft";
import { timelineItem } from "./build";
import { conversationMarkdown, exportFileName } from "./export";

let next = 0;
const item = (body: ItemBody, over: Partial<TimelineItem> = {}): TimelineItem =>
	timelineItem(body, { id: `i${next}`, ordinal: next++, ...over });

const you = (text: string, more: Partial<Extract<ItemBody, { kind: "user_message" }>> = {}) =>
	item({ kind: "user_message", text, attachments: [], delivery: "sent", ...more });

const reply = (text: string) => item({ kind: "assistant_message", text, streaming: false });

const read = (path: string) => item({ kind: "file_read", path, lines: null });

const file = (id: string, name: string, kind: AttachmentRef["kind"] = "image"): AttachmentRef =>
	({ id, name, kind, path: `/tmp/${name}`, bytes: 10, mime: "image/png" }) as AttachmentRef;

describe("conversationMarkdown", () => {
	it("heads each change of speaker and lists the steps between", () => {
		const text = conversationMarkdown(
			[
				you("Fix the login"),
				reply("Looking."),
				read("src/login.ts"),
				read("src/auth.ts"),
				reply("Fixed it."),
				you("Thanks"),
			],
			{ title: "Login bug", vendor: "claude" }
		);
		expect(text).toBe(
			[
				"# Login bug",
				"## You",
				"Fix the login",
				"## Claude",
				"Looking.",
				"- Read `src/login.ts`\n- Read `src/auth.ts`",
				"Fixed it.",
				"## You",
				"Thanks",
			].join("\n\n") + "\n"
		);
	});

	it("starts the agent's heading for steps with no reply before them", () => {
		const text = conversationMarkdown([you("Go"), read("a.ts"), you("Again")], {
			vendor: "codex",
		});
		expect(text).toBe("## You\n\nGo\n\n## Codex\n\n- Read `a.ts`\n\n## You\n\nAgain\n");
	});

	it("skips messages still queued or cancelled, and what it doesn't export", () => {
		const text = conversationMarkdown(
			[
				you("Sent"),
				you("Waiting", { delivery: "queued" }),
				you("Dropped", { delivery: "cancelled" }),
				item({ kind: "reasoning", text: "Hmm", streaming: false }),
				item({ kind: "turn_summary", duration_ms: 10, context_tokens: null }),
				reply("Done"),
			],
			{ vendor: "claude" }
		);
		expect(text).toBe("## You\n\nSent\n\n## Claude\n\nDone\n");
	});

	it("shows placed files as markers and names the rest", () => {
		const shot = file("f1", "shot.png");
		const notes = file("f2", "notes.txt", "file");
		const text = conversationMarkdown(
			[
				you(`See ${OBJECT_MARK} here`, {
					attachments: [shot, notes],
					refs: [{ type: "attachment", id: "f1" }],
				}),
			],
			{ vendor: "claude" }
		);
		expect(text).toBe("## You\n\nSee [Image 1: shot.png] here\n\nAttached: notes.txt\n");
	});

	it("gives a sub-agent's task and report, and leaves out its own steps", () => {
		const task = item({
			kind: "subagent",
			subagent: null,
			agent_type: "Explore",
			description: "Find the\nlogin code",
			result: "It's in src/login.ts.\n\nNothing else.",
		});
		const text = conversationMarkdown(
			[task, item({ kind: "file_read", path: "x.ts", lines: null }, { parent: task.id })],
			{ vendor: "claude" }
		);
		expect(text).toBe(
			"## Claude\n\n**Sub-agent, Explore:** Find the login code\n\n> It's in src/login.ts.\n>\n> Nothing else.\n"
		);
	});

	it("follows the agent to the vendor it moved to", () => {
		const text = conversationMarkdown(
			[
				reply("On Claude"),
				item({
					kind: "handoff",
					from_vendor: "claude",
					from_model: null,
					to_vendor: "codex",
					to_model: null,
				}),
				you("Carry on"),
				reply("On Codex"),
			],
			{ vendor: "codex" }
		);
		expect(text).toBe(
			"## Claude\n\nOn Claude\n\n*Context handoff, Claude to Codex*\n\n## You\n\nCarry on\n\n## Codex\n\nOn Codex\n"
		);
	});

	it("says Local for a move to or from a local model, which runs as Claude", () => {
		const move = (from_local: boolean, to_local: boolean) =>
			item({
				kind: "handoff",
				from_vendor: "claude",
				from_model: null,
				to_vendor: "claude",
				to_model: null,
				from_local,
				to_local,
			});
		const text = conversationMarkdown(
			[reply("On Claude"), move(false, true), reply("On Local"), move(true, false)],
			{ vendor: "claude" }
		);
		expect(text).toBe(
			"## Claude\n\nOn Claude\n\n*Context handoff, Claude to Local*\n\n## Local\n\nOn Local\n\n*Context handoff, Local to Claude*\n"
		);
	});

	it("keeps plans, sent files and errors", () => {
		const text = conversationMarkdown(
			[
				item({ kind: "plan", markdown: "1. Read\n2. Fix", streaming: false }),
				item({
					kind: "agent_attachments",
					text: "Here",
					attachments: [file("f", "a.png")],
				}),
				item({ kind: "error", message: "Out of\nmemory", retrying: false }),
				item({ kind: "error", message: "Retrying soon", retrying: true }),
			],
			{ vendor: "cursor" }
		);
		expect(text).toBe(
			"## Cursor\n\n**Plan**\n\n1. Read\n2. Fix\n\nHere\n\nSent: a.png\n\n*Error: Out of memory*\n"
		);
	});

	it("names a visual by its title and file", () => {
		const text = conversationMarkdown(
			[
				item({
					kind: "agent_visual",
					title: "Weekly\nusage",
					attachment: file("v", "visual-weekly-usage.html"),
					replaces: null,
				}),
			],
			{ vendor: "claude" }
		);
		expect(text).toBe("## Claude\n\n**Visual:** Weekly usage (`visual-weekly-usage.html`)\n");
	});
});

describe("exportFileName", () => {
	const day = new Date(2026, 10, 8, 23, 30);

	it("names the file after the agent and the day", () => {
		expect(exportFileName("Fix the login bug", day)).toBe("Fix the login bug 2026-11-08.md");
		expect(exportFileName("Fix the login bug", new Date(2026, 0, 2))).toBe(
			"Fix the login bug 2026-01-02.md"
		);
	});

	it("keeps out what a file name can't hold", () => {
		expect(exportFileName('Fix a/b: "why?"\n<now>|*', day)).toBe(
			"Fix a b why now 2026-11-08.md"
		);
		expect(exportFileName("..hidden", day)).toBe("hidden 2026-11-08.md");
		expect(exportFileName("x".repeat(200), day)).toBe(`${"x".repeat(80)} 2026-11-08.md`);
	});

	it("reads Conversation for a title with nothing left", () => {
		expect(exportFileName(undefined, day)).toBe("Conversation 2026-11-08.md");
		expect(exportFileName(" /?. ", day)).toBe("Conversation 2026-11-08.md");
	});
});
