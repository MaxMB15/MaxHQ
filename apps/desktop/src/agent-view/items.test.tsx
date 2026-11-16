// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { AUTO_CONSENT_KEY } from "@/settings/auto-consent";
import {
	DEVICE,
	fakeSource,
	heldAgent,
	installBrowserFakes,
	pillsIn,
	type,
} from "@/composer/testing";
import { mockSource } from "@/data/mock-source";
import { callerCalls } from "@maxhq/shared/data/mock-started";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import {
	ANSWER_PILLS_SINCE,
	DISMISS_SINCE,
	type AttachmentRef,
	type AttentionAnswer,
	type AttentionId,
	type InlineRef,
	type ItemBody,
	type ItemStatus,
	type TimelineItem,
} from "@maxhq/shared/bindings";
import { type AgentRef, OBJECT_MARK } from "@maxhq/shared/pills/draft";
import { formatClock } from "@maxhq/shared/status/format";
import {
	account,
	agent as agentOf,
	entities,
	link,
	MIN,
	NOW,
	workspace,
} from "@maxhq/shared/test/build";
import { applyUpdate } from "@maxhq/shared/timeline/apply";
import { ITEM_KINDS, timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { ItemBodyView } from "./item-row";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { moveSpec, moveTarget, roomLeft } from "./move";

beforeAll(installBrowserFakes);
afterEach(clearKept);

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

type Body<K extends ItemBody["kind"]> = Extract<ItemBody, { kind: K }>;

const show = (
	item: TimelineItem,
	row: Partial<RowContext> = {},
	source: DataSource = mockSource({ now: () => NOW, tick: false })
) => {
	const answer = vi.fn<
		(
			attention: AttentionId,
			answer: AttentionAnswer,
			refs?: readonly InlineRef[]
		) => Promise<ActionResult>
	>(async () => DONE);
	const value: RowContext = {
		agent: "a1",
		vendor: "claude",
		timeline: createTimelineStore(),
		readOnly: false,
		remote: false,
		answer,
		subAgentsShown: false,
		toggleSubAgents: () => {},
		...row,
	};
	const wrap = (children: ReactNode) => (
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>
				<RowProvider value={value}>{children}</RowProvider>
			</FixedNow>
		</ServicesProvider>
	);
	const view = render(wrap(<ItemBodyView item={item} />));
	return {
		answer,
		rerender: (next: TimelineItem) => view.rerender(wrap(<ItemBodyView item={next} />)),
	};
};

const item = (body: ItemBody, status: ItemStatus = "completed") =>
	timelineItem(body, {
		id: `item-${body.kind}`,
		status,
		started_at: NOW - 4000,
		completed_at: status === "completed" ? NOW : null,
	});

// A ready timeline holding these items, for rows that look at other items.
const storeOf = (items: TimelineItem[], hasOlder = false) => {
	const store = createTimelineStore();
	store.setState(
		applyUpdate(store.getState(), { type: "reset", seq: 1, items, has_older: hasOlder })
	);
	return store;
};

// One body of every kind, and text its renderer must show.
const SAMPLES: Record<ItemBody["kind"], [ItemBody, string | RegExp, ItemStatus?]> = {
	user_message: [
		{ kind: "user_message", text: "Ship the release notes", attachments: [], delivery: "sent" },
		"Ship the release notes",
	],
	assistant_message: [
		{ kind: "assistant_message", text: "Pushed the fix to **dev**.", streaming: false },
		/Pushed the fix to/,
	],
	reasoning: [
		{ kind: "reasoning", text: "Check the lockfile first.", streaming: false },
		/^Thought/,
	],
	command: [
		{
			kind: "command",
			command: "pnpm test:desktop",
			cwd: "/Users/max/Code/MaxHQ",
			exit_code: 0,
			output: "Tests 220 passed",
			output_truncated: false,
			background: false,
		},
		"pnpm test:desktop",
	],
	file_change: [
		{
			kind: "file_change",
			changes: [
				{
					path: "apps/desktop/src/App.tsx",
					old_path: null,
					op: "modify",
					additions: 3,
					deletions: 1,
					patch: null,
				},
			],
		},
		"apps/desktop/src/App.tsx",
	],
	file_read: [
		{ kind: "file_read", path: "crates/daemon/src/drive/mod.rs", lines: "10-40" },
		"crates/daemon/src/drive/mod.rs",
	],
	search: [
		{ kind: "search", query: "openTimeline", scope: "apps/desktop/src", matches: 4 },
		"openTimeline",
	],
	web: [
		{ kind: "web", action: "fetch", target: "https://v2.tauri.app/reference/config/" },
		"https://v2.tauri.app/reference/config/",
	],
	tool_call: [
		{
			kind: "tool_call",
			server: "github",
			name: "list_pull_requests",
			title: "Open pull requests",
			input: null,
			output: null,
		},
		"github:list_pull_requests",
	],
	subagent: [
		{
			kind: "subagent",
			subagent: null,
			agent_type: "Explore",
			description: "Find every caller of openTimeline",
			result: null,
		},
		"Find every caller of openTimeline",
	],
	plan: [
		{ kind: "plan", markdown: "1. Add the store\n2. Add the view", streaming: false },
		"Add the store",
	],
	todos: [
		{ kind: "todos", todos: [{ text: "Write the bench", state: "in_progress" }] },
		"Write the bench",
	],
	approval: [
		{
			kind: "approval",
			attention: "att-sample",
			approval_kind: "command",
			title: "Run `pnpm install`",
			detail: "pnpm install",
			cwd: null,
			options: [{ id: "allow_once", label: "Allow once" }],
			decided: null,
		},
		"Allow once",
		"waiting",
	],
	question: [
		{
			kind: "question",
			attention: "att-sample-q",
			questions: [
				{
					id: "q1",
					header: null,
					text: "Which branch should I merge into?",
					options: [],
					multi: false,
					other: true,
					secret: false,
				},
			],
			answers: null,
		},
		"Which branch should I merge into?",
		"waiting",
	],
	error: [
		{ kind: "error", message: "The API returned 529, overloaded.", retrying: true },
		"The API returned 529, overloaded.",
	],
	limit_wait: [
		{ kind: "limit_wait", account: null, resets_at: NOW + 30 * MIN, auto_resume: true },
		/^Waiting for usage limit reset/,
		"waiting",
	],
	compaction: [
		{ kind: "compaction", before_tokens: 180_000, after_tokens: 42_000 },
		/^Compacted the conversation/,
	],
	interrupted: [{ kind: "interrupted" }, "You stopped this turn"],
	turn_summary: [
		{ kind: "turn_summary", duration_ms: 95_000, context_tokens: 64_000 },
		/^Worked for/,
	],
	notice: [
		{ kind: "notice", level: "warning", message: "Hooks changed since the session started." },
		"Hooks changed since the session started.",
	],
	handoff: [
		{
			kind: "handoff",
			from_vendor: "claude",
			from_model: "opus",
			to_vendor: "codex",
			to_model: null,
		},
		"Context handoff pending",
		"pending",
	],
	agent_attachments: [
		{
			kind: "agent_attachments",
			text: "The chart you asked for.",
			attachments: [
				{
					id: "att-chart",
					name: "chart.png",
					kind: "image",
					bytes: 2048,
					mime: "image/png",
					path: "/attachments/chart.png",
					thumbnail: null,
				},
			],
		},
		"The chart you asked for.",
	],
	agent_opened_file: [
		{ kind: "agent_opened_file", path: "/tmp/example/docs/plan.md", line: 12, outside: false },
		"plan.md",
	],
	agent_visual: [
		{
			kind: "agent_visual",
			title: "Weekly usage",
			attachment: {
				id: "v1",
				name: "visual-weekly-usage.html",
				kind: "file",
				bytes: 900,
				mime: "text/html",
				path: "/attachments/visual-weekly-usage.html",
				thumbnail: null,
			},
			replaces: null,
		},
		"Weekly usage",
	],
	user_run: [
		{
			kind: "user_run",
			terminal: "terminal-1",
			command: "pnpm test",
			cwd: "~/code/example-app",
			device: "device-1",
			private: false,
			process: { state: "exited", code: 1, signal: null },
			duration_ms: 4200,
			lines: 214,
			output: "1 failed",
			output_truncated: false,
			moved: false,
		},
		"user_run",
		"failed",
	],
	unknown: [{ kind: "unknown", native_kind: "hook_progress" }, "hook_progress"],
};

describe("item renderers", () => {
	it("cover every kind", () => {
		expect(Object.keys(SAMPLES).sort()).toEqual([...ITEM_KINDS].sort());
	});

	it.each(ITEM_KINDS)("draw a %s item", (kind) => {
		const [body, text, status] = SAMPLES[kind];
		show(item(body, status));
		expect(screen.getAllByText(text).length).toBeGreaterThan(0);
	});

	it("name both vendors where the agent moved, and say when it carried the conversation", () => {
		const [body] = SAMPLES.handoff;
		const { rerender } = show(item(body, "pending"));
		expect(screen.getByText("Claude opus")).toBeInTheDocument();
		expect(screen.getByText("Codex")).toBeInTheDocument();
		rerender(item(body));
		expect(screen.getByText("Context handoff")).toBeInTheDocument();
		rerender(item(body, "cancelled"));
		expect(screen.getByText("Context handoff skipped")).toBeInTheDocument();
	});

	it("read a call to the agent tools in words", async () => {
		const [start, , wait] = callerCalls();
		show(item(start!));
		expect(await screen.findByText("Write the release notes on devbox")).toBeInTheDocument();
		expect(screen.getByText("Started")).toBeInTheDocument();
		cleanup();
		show(item(wait!, "running"));
		expect(screen.getByText("Waiting on")).toBeInTheDocument();
		expect(screen.getByText("2 agents")).toBeInTheDocument();
	});

	it("copy a finished reply as Markdown, and offer no copy while it streams", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		try {
			const text = "Pushed the fix to **dev**.";
			const streaming = item({ kind: "assistant_message", text, streaming: true }, "running");
			const { rerender } = show(streaming);
			expect(screen.queryByRole("button", { name: /Copy/ })).toBeNull();
			rerender(item({ kind: "assistant_message", text, streaming: false }));
			fireEvent.click(screen.getByRole("button", { name: "Copy" }));
			expect(writeText).toHaveBeenCalledWith(text);
			expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
		} finally {
			Reflect.deleteProperty(navigator, "clipboard");
		}
	});

	it("show a message still queued with its place in the queue", () => {
		show(
			item(
				{
					kind: "user_message",
					text: "Then bump the version",
					attachments: [],
					delivery: "queued",
				},
				"pending"
			)
		);
		expect(screen.getByText("Then bump the version")).toBeInTheDocument();
	});

	it("read a message you sent as Markdown", () => {
		show(
			item({
				kind: "user_message",
				text: "A **bold** word and `code`\nnext line <Foo>\n\n```\nlong code\n```",
				attachments: [],
				delivery: "sent",
			})
		);
		expect(screen.getByText("bold").tagName).toBe("STRONG");
		expect(screen.getByText("code").tagName).toBe("CODE");
		expect(document.querySelector("br")).not.toBeNull();
		expect(document.body).toHaveTextContent("next line <Foo>");
		expect(document.body).toHaveTextContent("long code");
		expect(document.body).not.toHaveTextContent("**");
	});

	it("keep a pill in its place inside a message's Markdown", () => {
		show(
			item({
				kind: "user_message",
				text: `Look at **this ${OBJECT_MARK}** now`,
				attachments: [
					{
						id: "att-notes",
						name: "notes.md",
						kind: "file",
						bytes: 12,
						mime: null,
						path: "/staged/notes.md",
						thumbnail: null,
					},
				],
				refs: [{ type: "attachment", id: "att-notes" }],
				delivery: "sent",
			})
		);
		const strong = screen.getByText(/^this/);
		expect(strong.tagName).toBe("STRONG");
		expect(strong.querySelector("[data-piece]")).toHaveTextContent("notes.md");
		expect(document.querySelector("[data-find='markdown']")).not.toBeNull();
	});

	it("show the preview of a picture you sent, and a frame for one without", () => {
		const preview = "data:image/jpeg;base64,/9j/4AAQSkZJRg==";
		const picture = (name: string, thumbnail: string | null): AttachmentRef => ({
			id: `att-${name}`,
			name,
			kind: "image",
			bytes: 2048,
			mime: "image/png",
			path: `/staged/${name}`,
			thumbnail,
		});
		show(
			item({
				kind: "user_message",
				text: "Like this",
				attachments: [picture("shot.png", preview), picture("before.png", null)],
				delivery: "sent",
			})
		);
		const shot = screen.getByRole("button", { name: "Show shot.png full size" });
		expect(shot.querySelector("img")).toHaveAttribute("src", preview);
		const before = screen.getByRole("button", { name: "Show before.png full size" });
		expect(before.querySelector("img")).toBeNull();
		expect(before).toHaveTextContent("before.png");
	});

	it("show the files an agent sent you as its own, with their previews", () => {
		const preview = "data:image/png;base64,iVBORw0KGgo=";
		show(
			item({
				kind: "agent_attachments",
				text: "Both **charts**.",
				attachments: [
					{
						id: "att-plot",
						name: "plot.png",
						kind: "image",
						bytes: 2048,
						mime: "image/png",
						path: "/attachments/plot.png",
						thumbnail: preview,
					},
					{
						id: "att-data",
						name: "data.csv",
						kind: "file",
						bytes: 900,
						mime: "text/csv",
						path: "/attachments/data.csv",
						thumbnail: null,
					},
				],
			})
		);
		const plot = screen.getByRole("button", { name: "Show plot.png full size" });
		expect(plot.querySelector("img")).toHaveAttribute("src", preview);
		expect(screen.getByText("data.csv")).toBeInTheDocument();
		expect(screen.getByText("charts").tagName).toBe("STRONG");
		expect(screen.getByText(/^Sent you 2 files/)).toBeInTheDocument();
	});
});

const OPTIONS = [
	{ id: "allow_once", label: "Allow once" },
	{ id: "allow_always", label: "Always allow pnpm" },
	{ id: "deny", label: "Deny" },
];

const approval = (over: Partial<Extract<ItemBody, { kind: "approval" }>> = {}) =>
	item(
		{
			kind: "approval",
			attention: "att-approval",
			approval_kind: "command",
			title: "Run `pnpm install`",
			detail: "pnpm install --frozen-lockfile",
			cwd: null,
			options: OPTIONS,
			decided: null,
			...over,
		},
		over.decided ? "completed" : "waiting"
	);

describe("approval card", () => {
	// Sets how tall the detail box's text runs against what the box shows.
	const tall = (scrollHeight: number, clientHeight: number) => {
		const spies = [
			vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(scrollHeight),
			vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(clientHeight),
		];
		return () => spies.forEach((spy) => spy.mockRestore());
	};

	it("says when the detail runs past what its box shows", () => {
		const restore = tall(900, 220);
		show(approval());
		expect(screen.getByText("More below. Scroll to read it all.")).toBeInTheDocument();
		restore();
	});

	it("drops that once you've scrolled to the end", () => {
		const restore = tall(900, 220);
		show(approval());
		const box = screen.getByText("$").closest("pre") as HTMLElement;
		box.scrollTop = 680;
		fireEvent.scroll(box);
		expect(screen.queryByText("More below. Scroll to read it all.")).toBeNull();
		restore();
	});

	it("says nothing when the detail fits", () => {
		const restore = tall(100, 100);
		show(approval());
		expect(screen.queryByText("More below. Scroll to read it all.")).toBeNull();
		restore();
	});

	it("spells out what the title, detail and options would hide", () => {
		show(
			approval({
				title: "Run `rm\u202e`?",
				detail: "rm -rf ~\u200b",
				options: [
					{ id: "allow_once", label: "Allow\u2066 once" },
					{ id: "deny", label: "Deny" },
				],
			})
		);
		expect(screen.getByText("rm[U+202E]")).toBeInTheDocument();
		expect(screen.getByText(/rm -rf ~\[U\+200B\]/)).toBeInTheDocument();
		expect(screen.getByRole("button", { name: /Allow\[U\+2066\] once/ })).toBeInTheDocument();
	});

	it("puts a yes that lasts behind the one for this request", () => {
		const { answer } = show(
			approval({
				options: [
					{ id: "allow_always", label: "Allow always" },
					{ id: "allow_once", label: "Allow once" },
					{ id: "deny", label: "Don't allow" },
				],
			})
		);
		const card = screen.getByRole("region", { name: "Permission request" });
		fireEvent.keyDown(card, { key: "Enter" });
		expect(answer).toHaveBeenCalledWith("att-approval", { type: "option", id: "allow_once" });
	});

	it("sends the main option", async () => {
		const { answer } = show(approval());
		fireEvent.click(screen.getByRole("button", { name: /Allow once/ }));
		expect(answer).toHaveBeenCalledWith("att-approval", { type: "option", id: "allow_once" });
		await waitFor(() =>
			expect(screen.getByRole("button", { name: /Allow once/ })).toBeEnabled()
		);
	});

	it("can take the focus, so Enter sends the main option", () => {
		const { answer } = show(approval());
		const card = screen.getByRole("region", { name: "Permission request" });
		card.focus();
		expect(card).toHaveFocus();
		fireEvent.keyDown(card, { key: "Enter" });
		expect(answer).toHaveBeenCalledWith("att-approval", { type: "option", id: "allow_once" });
	});

	it("sends another option that says yes", () => {
		const { answer } = show(approval());
		fireEvent.click(screen.getByRole("button", { name: "Always allow pnpm" }));
		expect(answer).toHaveBeenCalledWith("att-approval", { type: "option", id: "allow_always" });
	});

	it("sends your reason when you deny with one", async () => {
		const { answer } = show(approval());
		const reason = screen.getByLabelText("Reason for denying");
		expect(reason).toHaveAttribute("spellcheck", "true");
		await type(reason, "  Use pnpm add instead ");
		fireEvent.click(screen.getByRole("button", { name: "Deny" }));
		expect(answer).toHaveBeenCalledWith(
			"att-approval",
			{ type: "text", text: "Use pnpm add instead" },
			[]
		);
	});

	it("denies with Enter in the reason, and Shift+Enter breaks the line", async () => {
		const { answer } = show(approval());
		const reason = screen.getByLabelText("Reason for denying");
		await type(reason, "Not now");
		fireEvent.keyDown(reason, { key: "Enter", shiftKey: true });
		expect(answer).not.toHaveBeenCalled();
		fireEvent.keyDown(reason, { key: "Enter" });
		expect(answer).toHaveBeenCalledWith("att-approval", { type: "text", text: "Not now" }, []);
	});

	it("sends the deny option when you give no reason", () => {
		const { answer } = show(approval());
		fireEvent.click(screen.getByRole("button", { name: "Deny" }));
		expect(answer).toHaveBeenCalledWith("att-approval", { type: "option", id: "deny" });
	});

	it("says decline for Codex", () => {
		show(
			approval({
				options: [
					{ id: "accept", label: "Yes" },
					{ id: "decline", label: "No" },
				],
			}),
			{
				vendor: "codex",
			}
		);
		expect(screen.getByRole("region", { name: "Approval request" })).toBeInTheDocument();
		expect(screen.getByLabelText("Reason for declining")).toBeInTheDocument();
	});

	it("shows why an answer didn't go through", async () => {
		const { answer } = show(approval());
		answer.mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "the agent already moved on" },
		});
		fireEvent.click(screen.getByRole("button", { name: /Allow once/ }));
		expect(await screen.findByRole("alert")).toHaveTextContent("The agent already moved on.");
	});

	it("only lists the options for a session in a terminal", () => {
		const { answer } = show(approval(), { readOnly: true });
		expect(screen.queryByRole("button")).not.toBeInTheDocument();
		expect(screen.getByText(/Answer it in the terminal/)).toBeInTheDocument();
		expect(screen.getByRole("list", { name: "Options" })).toHaveTextContent(
			"Always allow pnpm"
		);
		expect(answer).not.toHaveBeenCalled();
	});

	it("shows who answered once decided from MaxHQ", () => {
		const at = NOW - 2 * MIN;
		show(
			approval({
				decided: {
					answer: { type: "option", id: "allow_once" },
					by: "client-devs-macbook",
					at,
				},
			})
		);
		expect(
			screen.getByText(`Allow once, from MaxHQ at ${formatClock(at)}`)
		).toBeInTheDocument();
		expect(screen.getByRole("region", { name: "Request allowed" })).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: /Allow once/ })).not.toBeInTheDocument();
	});

	it("shows an answer given in the terminal, and the reason for a no", () => {
		const at = NOW - MIN;
		show(
			approval({
				decided: { answer: { type: "text", text: "Not on main" }, by: null, at },
			})
		);
		expect(
			screen.getByText(`Answered with a note, in the terminal at ${formatClock(at)}`)
		).toBeInTheDocument();
		expect(screen.getByRole("region", { name: "Request turned down" })).toBeInTheDocument();
		expect(screen.getByText("Not on main")).toBeInTheDocument();
	});

	it("turns into a decided card when the answer arrives", () => {
		const { rerender } = show(approval());
		rerender(
			approval({ decided: { answer: { type: "option", id: "deny" }, by: null, at: NOW } })
		);
		expect(
			screen.getByText(`Deny, in the terminal at ${formatClock(NOW)}`)
		).toBeInTheDocument();
	});

	it("says which folder the command runs in, and on which device", async () => {
		const command = timelineItem(
			{ ...(SAMPLES.command[0] as Body<"command">), cwd: "/Users/max/Code/MaxHQ" },
			{ id: "cmd-1", status: "pending" }
		);
		const asked = { ...approval(), parent: "cmd-1", ordinal: 1 };
		show(asked, { agent: "agent-release", timeline: storeOf([command, asked]) });
		const folder = await screen.findByText("In MaxHQ on Mac.");
		expect(folder).toHaveAttribute("title", "/Users/max/Code/MaxHQ");
	});

	it("shows the folder the approval names before its command's", async () => {
		const command = timelineItem(
			{ ...(SAMPLES.command[0] as Body<"command">), cwd: "/Users/max/Code/MaxHQ" },
			{ id: "cmd-1", status: "pending" }
		);
		const asked = {
			...approval({ cwd: "/Users/max/Code/MaxHQ/crates/daemon" }),
			parent: "cmd-1",
			ordinal: 1,
		};
		show(asked, { agent: "agent-release", timeline: storeOf([command, asked]) });
		const folder = await screen.findByText("In daemon on Mac.");
		expect(folder).toHaveAttribute("title", "/Users/max/Code/MaxHQ/crates/daemon");
	});

	it("shows the folder an approval names without a command", async () => {
		show(approval({ approval_kind: "file_change", cwd: "/Users/max/Code/MaxHQ" }), {
			agent: "agent-release",
		});
		expect(await screen.findByText("In MaxHQ on Mac.")).toBeInTheDocument();
	});

	it("leaves the folder out when the approval isn't for a command it can see", () => {
		show(approval(), { agent: "agent-release" });
		expect(screen.queryByText(/^In /)).not.toBeInTheDocument();
	});
});

const subagent = (
	over: Partial<Body<"subagent">> = {},
	status: ItemStatus = "running",
	took = 38_000
) =>
	timelineItem(
		{
			kind: "subagent",
			subagent: null,
			agent_type: "Explore",
			description: "find usage call sites",
			result: null,
			...over,
		},
		{
			id: "task-1",
			status,
			started_at: NOW - took,
			completed_at: status === "running" ? null : NOW,
		}
	);

// A tool step the sub-agent made, nested under its card.
const step = (n: number, body: ItemBody, status: ItemStatus = "completed") =>
	timelineItem(body, { id: `step-${n}`, parent: "task-1", ordinal: n, status });

const STEPS = [
	step(1, { kind: "file_read", path: "src/header/usage-popover.tsx", lines: null }),
	step(2, { kind: "search", query: "window_minutes", scope: "src", matches: 7 }),
	step(
		3,
		{ ...(SAMPLES.command[0] as Body<"command">), command: "pnpm test -- usage" },
		"running"
	),
];

describe("sub-agent card", () => {
	it("counts the tool calls nested under it and names the one it's on", () => {
		const card = subagent();
		show(card, { timeline: storeOf([card, ...STEPS]) });
		expect(screen.getByText(/^Sub-agent, 3 tool calls, .*\. Now:$/)).toBeInTheDocument();
		expect(screen.getByText("pnpm test -- usage")).toHaveAttribute(
			"title",
			"pnpm test -- usage"
		);
	});

	it("says how many calls it took once done", () => {
		const card = subagent({}, "completed");
		const done = STEPS.map((s) => ({ ...s, status: "completed" as const }));
		show(card, { timeline: storeOf([card, ...done]) });
		expect(screen.getByText("Sub-agent, 3 tool calls, took 38 s")).toBeInTheDocument();
		expect(screen.queryByText(/Now:/)).not.toBeInTheDocument();
	});

	it("says how far a failed one got", () => {
		const card = subagent({}, "failed");
		show(card, { timeline: storeOf([card, ...STEPS.slice(0, 2)]) });
		expect(screen.getByText("Sub-agent, failed after 2 tool calls")).toBeInTheDocument();
	});

	it("keeps the plain line when it has no steps to count", () => {
		show(subagent({}, "completed"));
		expect(screen.getByText("Sub-agent, took 38 s")).toBeInTheDocument();
	});

	it("reads a sub-agent's own timeline when it has one", async () => {
		show(subagent({ subagent: "agent-w1" }), { agent: "agent-m1-lead" });
		expect(await screen.findByText("cargo test -p maxhq-daemon replica")).toBeInTheDocument();
		expect(screen.getByText(/^Sub-agent, 5 tool calls, .*\. Now:$/)).toBeInTheDocument();
	});

	it("reads as shown only while the panel points out its own sub-agent", () => {
		const toggle = vi.fn<(subAgent: string) => void>();
		const card = subagent({ subagent: "agent-w1" });
		show(card, {
			agent: "agent-m1-lead",
			subAgentsShown: true,
			shownSubAgent: "agent-w2",
			toggleSubAgents: toggle,
		});
		const button = screen.getByRole("button", { name: "Show in panel" });
		expect(button).toHaveAttribute("aria-pressed", "false");
		fireEvent.click(button);
		expect(toggle).toHaveBeenCalledWith("agent-w1");

		cleanup();
		show(card, { agent: "agent-m1-lead", subAgentsShown: true, shownSubAgent: "agent-w1" });
		expect(screen.getByRole("button", { name: "Shown in panel" })).toHaveAttribute(
			"aria-pressed",
			"true"
		);
	});

	it("counts nested steps in full while older items are still to load", () => {
		const card = subagent({}, "completed");
		const steps = STEPS.map((s) => ({ ...s, status: "completed" as const }));
		show(card, { timeline: storeOf([card, ...steps], true) });
		// Steps nested under a card come after it, so they're all here.
		expect(screen.getByText("Sub-agent, 3 tool calls, took 38 s")).toBeInTheDocument();
	});
});

describe("limit wait card", () => {
	// The sample Codex agent on Codex Plus, which is at its 5 hour limit.
	// Claude Max on the same Mac has 78% of its 5 hours left.
	const step = (id: string, body: ItemBody, ordinal: number, status: ItemStatus = "completed") =>
		timelineItem(body, {
			id,
			agent: "agent-hdr",
			ordinal,
			status,
			started_at: NOW - (10 - ordinal) * MIN,
			completed_at: status === "completed" ? NOW - (10 - ordinal) * MIN : null,
		});
	const user = (text: string, delivery: "sent" | "queued" = "sent"): ItemBody => ({
		kind: "user_message",
		text,
		attachments: [],
		delivery,
	});
	const wait = (autoResume = true) =>
		step(
			"wait",
			{
				kind: "limit_wait",
				account: "acct-codex-mac",
				resets_at: NOW + 2 * 60 * MIN,
				auto_resume: autoResume,
			},
			3,
			"running"
		);
	const HISTORY = [
		step("u1", user("Make tone mapping configurable."), 0),
		step(
			"a1",
			{
				kind: "assistant_message",
				text: "Tone mapping is now on, with `auto` as the default.",
				streaming: false,
			},
			1
		),
		step("u2", user("Keep going, the peak test is fine to loosen."), 2),
	];
	const QUEUED = step(
		"q1",
		user("Then add the option to Playback settings.", "queued"),
		4,
		"pending"
	);

	// The Mac starts agents in Auto, which you confirmed long ago.
	beforeEach(() => localStorage.setItem(AUTO_CONSENT_KEY, "confirmed"));

	const setup = (items: TimelineItem[], readOnly = false) => {
		const source = mockSource({ now: () => NOW, tick: false });
		const card = items.find((i) => i.kind === "limit_wait")!;
		const timeline = storeOf(items);
		const { rerender } = show(
			card,
			{ agent: "agent-hdr", vendor: "codex", timeline, readOnly },
			source
		);
		// What maxd sends once the waiting message is taken back: the message
		// cancelled, and the wait no longer resuming.
		const takenBack = () => {
			const cancelled = step(
				"q1",
				{
					kind: "user_message",
					text: "Then add the option to Playback settings.",
					attachments: [],
					delivery: "cancelled",
				},
				4,
				"cancelled"
			);
			act(() => {
				timeline.setState(
					applyUpdate(timeline.getState(), {
						type: "events",
						seq: 2,
						events: [{ type: "upsert", item: cancelled }],
					})
				);
			});
			rerender(wait(false));
		};
		return { source, takenBack };
	};

	it("moves the agent to an account with room, with a brief of where it got to", async () => {
		const { source, takenBack } = setup([...HISTORY, wait(), QUEUED]);
		const startAgent = vi.spyOn(source, "startAgent");
		const cancelQueued = vi.spyOn(source, "cancelQueued");
		const move = await screen.findByRole("button", { name: "Move to Claude Max" });
		expect(screen.getByText("Claude Max has 78% of its 5 hours left")).toBeInTheDocument();

		fireEvent.click(move);
		await waitFor(() => expect(startAgent).toHaveBeenCalledTimes(1));
		const spec = startAgent.mock.calls[0]![0];
		expect(spec).toMatchObject({
			vendor: "claude",
			account: "acct-claude-mac",
			place: { type: "workspace", workspace: "ws-mac-comet" },
			model: null,
			permission: null,
			title: "HDR tone mapping",
		});
		expect(spec.first?.text).toBe(
			[
				'You\'re taking over from a Codex agent called "HDR tone mapping" in this workspace. It stopped because Codex Plus hit its usage limit.',
				"The last things it was asked:\n- Make tone mapping configurable.\n- Keep going, the peak test is fine to loosen.",
				"The last thing it said:\nTone mapping is now on, with `auto` as the default.",
				"Messages that were waiting for it:\n- Then add the option to Playback settings.",
				"Check the workspace for its changes, then carry on from there.",
			].join("\n\n")
		);
		// The new agent has the waiting message, so this one drops it.
		await waitFor(() => expect(cancelQueued).toHaveBeenCalledWith("agent-hdr", "q1"));
		await waitFor(() =>
			expect(screen.getByRole("button", { name: "Move to Claude Max" })).toBeEnabled()
		);
		takenBack();
		expect(
			screen.getByText(/Send a message once it resets to carry on\.$/)
		).toBeInTheDocument();
		expect(screen.queryByRole("alert")).not.toBeInTheDocument();
	});

	it("says why the move failed, and keeps the waiting messages", async () => {
		const { source } = setup([...HISTORY, wait(), QUEUED]);
		vi.spyOn(source, "startAgent").mockResolvedValueOnce({
			status: "error",
			error: { kind: "not_connected", device: "Mac" },
		});
		const cancelQueued = vi.spyOn(source, "cancelQueued");
		fireEvent.click(await screen.findByRole("button", { name: "Move to Claude Max" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't start an agent on Claude Max. Mac isn't connected."
		);
		expect(cancelQueued).not.toHaveBeenCalled();
		expect(screen.getByRole("button", { name: "Move to Claude Max" })).toBeEnabled();
	});

	it("stops the resume by taking back the waiting messages", async () => {
		const { source, takenBack } = setup([...HISTORY, wait(), QUEUED]);
		const cancelQueued = vi.spyOn(source, "cancelQueued");
		expect(screen.getByText(/This agent continues on its own in 2 h/)).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Don't resume" }));
		expect(cancelQueued).toHaveBeenCalledWith("agent-hdr", "q1");
		await waitFor(() =>
			expect(screen.getByRole("button", { name: "Don't resume" })).toBeEnabled()
		);
		takenBack();
		expect(
			screen.getByText(/Send a message once it resets to carry on\.$/)
		).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Don't resume" })).not.toBeInTheDocument();
	});

	it("says it resumes for as long as maxd says so", async () => {
		const { source } = setup([...HISTORY, wait(), QUEUED]);
		const cancelQueued = vi.spyOn(source, "cancelQueued");
		fireEvent.click(screen.getByRole("button", { name: "Don't resume" }));
		await waitFor(() => expect(cancelQueued).toHaveBeenCalledTimes(1));
		// The take back went through, but the card waits for maxd to clear
		// the resume rather than run ahead of it.
		await waitFor(() =>
			expect(screen.getByRole("button", { name: "Don't resume" })).toBeEnabled()
		);
		expect(screen.getByText(/This agent continues on its own in 2 h/)).toBeInTheDocument();
	});

	it("says why a message couldn't be taken back", async () => {
		const { source } = setup([...HISTORY, wait(), QUEUED]);
		vi.spyOn(source, "cancelQueued").mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "That message already went to Codex." },
		});
		fireEvent.click(screen.getByRole("button", { name: "Don't resume" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't take back the waiting messages. That message already went to Codex."
		);
		expect(screen.getByText(/This agent continues on its own/)).toBeInTheDocument();
	});

	it("offers only the move when nothing waits to resume it", async () => {
		setup([...HISTORY, wait(false)]);
		expect(await screen.findByRole("button", { name: "Move to Claude Max" })).toBeEnabled();
		expect(screen.queryByRole("button", { name: "Don't resume" })).not.toBeInTheDocument();
	});

	it("offers nothing for a session in a terminal", async () => {
		setup([...HISTORY, wait(), QUEUED], true);
		expect(await screen.findByText(/^Codex Plus is at its 5-hour limit\./)).toBeInTheDocument();
		expect(screen.queryByRole("button")).not.toBeInTheDocument();
	});

	it("moves only to an account on the same device with room left", () => {
		const full = account({
			id: "full",
			usage: {
				checked_at: NOW,
				source: "claude_cache",
				windows: [{ scope: null, window_minutes: 300, used_percent: 100, resets_at: null }],
			},
		});
		const roomy = account({
			id: "roomy",
			usage: {
				checked_at: NOW,
				source: "claude_cache",
				windows: [{ scope: null, window_minutes: 300, used_percent: 40, resets_at: null }],
			},
		});
		const elsewhere = account({ id: "elsewhere", device: "dev-node" });
		const own = account({ id: "own" });
		const accounts = [full, own, roomy, elsewhere];
		expect(moveTarget(accounts, { account: "own" }, "dev-mac")?.id).toBe("roomy");
		expect(moveTarget([full, own, elsewhere], { account: "own" }, "dev-mac")).toBeUndefined();
		expect(roomLeft(roomy)).toBe("60% of its 5 hours");
	});

	it("keeps the mode the agent runs in, unless the new CLI lacks it", () => {
		const claude = account({ id: "claude" });
		const codex = account({ id: "codex", vendor: "codex", plan: "plus" });
		const planning = agentOf({
			id: "a1",
			permission: { requested: "ask", effective: "plan" },
		});
		expect(moveSpec(planning, claude, "brief").permission).toBe("plan");
		expect(moveSpec(planning, codex, "brief").permission).toBeNull();
	});
});

const question = (
	questions: Extract<ItemBody, { kind: "question" }>["questions"],
	answers: Extract<ItemBody, { kind: "question" }>["answers"] = null
) =>
	item(
		{ kind: "question", attention: "att-question", questions, answers },
		answers ? "completed" : "waiting"
	);

const BRANCH = {
	id: "branch",
	header: "Branch",
	text: "Which branch should I merge into?",
	options: [
		{ label: "dev", description: "Where finished work goes" },
		{ label: "main", description: null },
	],
	multi: false,
	other: false,
	secret: false,
};

/** A question with no options, answered in your own words. */
const OPEN = { ...BRANCH, options: [], other: true, header: null };

const CHECKS = {
	id: "checks",
	header: null,
	text: "Which checks should run first?",
	options: [
		{ label: "typecheck", description: null },
		{ label: "lint", description: null },
		{ label: "tests", description: null },
	],
	multi: true,
	other: true,
	secret: false,
};

describe("question card", () => {
	it("sends the option you picked", () => {
		const { answer } = show(question([BRANCH]));
		const send = screen.getByRole("button", { name: /Send answer/ });
		expect(send).toBeDisabled();
		fireEvent.click(screen.getByRole("radio", { name: /dev/ }));
		expect(screen.getByRole("radio", { name: /dev/ })).toHaveAttribute("aria-checked", "true");
		fireEvent.click(send);
		expect(answer).toHaveBeenCalledWith(
			"att-question",
			{ type: "questions", answers: [{ id: "branch", values: ["dev"] }] },
			[]
		);
	});

	it("waits for every question, and sends picks in the listed order with your own answer", async () => {
		const { answer } = show(question([BRANCH, CHECKS]));
		fireEvent.click(screen.getByRole("checkbox", { name: "tests" }));
		fireEvent.click(screen.getByRole("checkbox", { name: "typecheck" }));
		await type(screen.getByLabelText("Your own answer"), "cargo deny");
		expect(screen.getByRole("button", { name: /Send answers/ })).toBeDisabled();
		fireEvent.click(screen.getByRole("radio", { name: /main/ }));
		fireEvent.click(screen.getByRole("button", { name: /Send answers/ }));
		expect(answer).toHaveBeenCalledWith(
			"att-question",
			{
				type: "questions",
				answers: [
					{ id: "branch", values: ["main"] },
					{ id: "checks", values: ["typecheck", "tests", "cargo deny"] },
				],
			},
			[]
		);
	});

	it("picks options with 1 to 9 on the focused card, then sends with Enter", () => {
		const { answer } = show(question([BRANCH, CHECKS]));
		const card = screen.getByRole("region", { name: "Questions" });
		card.focus();
		// 2 picks main and moves on to the checks, where 1 and 3 tick two boxes.
		fireEvent.keyDown(card, { key: "2" });
		fireEvent.keyDown(card, { key: "1" });
		fireEvent.keyDown(card, { key: "3" });
		// Past the last option, and with Command held, a digit does nothing.
		fireEvent.keyDown(card, { key: "4" });
		fireEvent.keyDown(card, { key: "2", metaKey: true });
		expect(screen.getByRole("radio", { name: /main/ })).toHaveAttribute("aria-checked", "true");
		expect(screen.getByRole("checkbox", { name: /typecheck/ })).toHaveAttribute(
			"aria-checked",
			"true"
		);
		expect(screen.getByRole("checkbox", { name: /lint/ })).toHaveAttribute(
			"aria-checked",
			"false"
		);
		fireEvent.keyDown(card, { key: "Enter" });
		expect(answer).toHaveBeenCalledWith(
			"att-question",
			{
				type: "questions",
				answers: [
					{ id: "branch", values: ["main"] },
					{ id: "checks", values: ["typecheck", "tests"] },
				],
			},
			[]
		);
	});

	it("picks for the question you click in, and leaves digits typed in a box alone", () => {
		show(question([BRANCH, CHECKS]));
		const card = screen.getByRole("region", { name: "Questions" });
		fireEvent.pointerDown(screen.getByRole("checkbox", { name: /lint/ }));
		fireEvent.keyDown(card, { key: "2" });
		expect(screen.getByRole("checkbox", { name: /lint/ })).toHaveAttribute(
			"aria-checked",
			"true"
		);
		fireEvent.keyDown(screen.getByLabelText("Your own answer"), { key: "1" });
		expect(screen.getByRole("checkbox", { name: /typecheck/ })).toHaveAttribute(
			"aria-checked",
			"false"
		);
		expect(screen.getByRole("radio", { name: /dev/ })).toHaveAttribute("aria-checked", "false");
	});

	it("takes a typed answer where there are no options, with spell check", async () => {
		const { answer } = show(question([OPEN]));
		const box = screen.getByLabelText("Your answer");
		expect(box).toHaveAttribute("spellcheck", "true");
		await type(box, "release/0.2");
		fireEvent.click(screen.getByRole("button", { name: /Send answer/ }));
		expect(answer).toHaveBeenCalledWith(
			"att-question",
			{ type: "questions", answers: [{ id: "branch", values: ["release/0.2"] }] },
			[]
		);
	});

	it("sends with Enter in the box for your own answer", async () => {
		const { answer } = show(question([OPEN]));
		const box = screen.getByLabelText("Your answer");
		await type(box, "release/0.2");
		fireEvent.keyDown(box, { key: "Enter" });
		expect(answer).toHaveBeenCalledWith(
			"att-question",
			{ type: "questions", answers: [{ id: "branch", values: ["release/0.2"] }] },
			[]
		);
	});

	it("can't be answered for a session in a terminal", () => {
		show(question([BRANCH]), { readOnly: true });
		expect(screen.getByRole("radio", { name: /dev/ })).toBeDisabled();
		expect(screen.queryByRole("button", { name: /Send answer/ })).not.toBeInTheDocument();
		expect(screen.getByText(/Answer it in the terminal/)).toBeInTheDocument();
	});

	it("says it's the agent asking when a question wants a secret", () => {
		const note = "The agent is asking for this, not MaxHQ.";
		show(question([{ ...CHECKS, id: "token", text: "Paste the token", secret: true }]));
		expect(screen.getByLabelText("Your own answer")).toHaveAttribute("type", "password");
		expect(screen.getByText(note)).toBeInTheDocument();
		cleanup();
		show(question([CHECKS]));
		expect(screen.queryByText(note)).not.toBeInTheDocument();
	});

	it("shows the answers once given, hiding a secret one", () => {
		show(
			question(
				[BRANCH, { ...CHECKS, id: "token", text: "Paste the token", secret: true }],
				[
					{ id: "branch", values: ["dev"] },
					{ id: "token", values: ["hunter2"] },
				]
			)
		);
		expect(screen.getByText("Answered the questions")).toBeInTheDocument();
		expect(screen.getByText("dev")).toBeInTheDocument();
		expect(screen.getByText("Hidden")).toBeInTheDocument();
		expect(screen.queryByText("hunter2")).not.toBeInTheDocument();
	});
});

describe("dismissing a question", () => {
	const DISMISSED = { type: "dismiss" };

	it("skips the questions from its button, which says what it does", async () => {
		const { answer } = show(question([BRANCH, CHECKS]));
		const dismiss = screen.getByRole("button", { name: "Dismiss" });
		expect(dismiss).toHaveAttribute("aria-keyshortcuts", "Escape");
		act(() => dismiss.focus());
		expect(
			await screen.findByText("Skip these questions and stop the turn", { exact: false })
		).toHaveTextContent("Skip these questions and stop the turn Esc");
		fireEvent.click(dismiss);
		expect(answer).toHaveBeenCalledWith("att-question", DISMISSED, []);
		expect(answer).toHaveBeenCalledOnce();
	});

	it("says the turn is stopping once Dismiss went, not that an answer did", async () => {
		show(question([BRANCH]));
		fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
		expect(await screen.findByRole("status")).toHaveTextContent(
			"Dismissed. Stopping Claude's turn."
		);
		expect(screen.queryByText(/Answer sent/)).not.toBeInTheDocument();

		cleanup();
		show(question([BRANCH]));
		fireEvent.click(screen.getByRole("radio", { name: /dev/ }));
		fireEvent.click(screen.getByRole("button", { name: /Send answer/ }));
		expect(await screen.findByRole("status")).toHaveTextContent(
			"Answer sent. Waiting for Claude to pick it up."
		);
	});

	it("skips them with Esc on the card or in the box for your own answer", () => {
		const { answer } = show(question([OPEN]));
		const card = screen.getByRole("region", { name: "Question" });
		card.focus();
		// With Command held, Esc passes on.
		fireEvent.keyDown(card, { key: "Escape", metaKey: true });
		expect(answer).not.toHaveBeenCalled();
		fireEvent.keyDown(card, { key: "Escape" });
		expect(answer).toHaveBeenLastCalledWith("att-question", DISMISSED, []);

		cleanup();
		const again = show(question([OPEN]));
		fireEvent.keyDown(screen.getByLabelText("Your answer"), { key: "Escape" });
		expect(again.answer).toHaveBeenCalledWith("att-question", DISMISSED, []);
	});

	it("leaves an Esc the box used, or one typed while composing, alone", () => {
		const { answer } = show(question([OPEN]));
		const box = screen.getByLabelText("Your answer");
		box.addEventListener("keydown", (event) => event.preventDefault(), { once: true });
		fireEvent.keyDown(box, { key: "Escape" });
		fireEvent.keyDown(box, { key: "Escape", isComposing: true });
		expect(answer).not.toHaveBeenCalled();
	});

	it("says it needs a newer maxd, and leaves Esc alone, until the maxd takes it", async () => {
		const mac = (protocol: number) =>
			link({ id: "mac", name: "Mac", device: DEVICE, route: { type: "local" }, protocol });
		const { source, upsert } = fakeSource({
			agent: heldAgent(),
			links: [mac(DISMISS_SINCE - 1)],
			more: entities({ workspaces: [workspace({ id: "w1" })] }),
		});
		const { answer } = show(question([BRANCH]), {}, source);
		await waitFor(() =>
			expect(screen.getByRole("button", { name: "Dismiss" })).toHaveAccessibleDescription(
				"Dismiss needs a newer maxd on Mac."
			)
		);
		const dismiss = screen.getByRole("button", { name: "Dismiss" });
		expect(dismiss).toHaveAttribute("aria-disabled", "true");
		fireEvent.click(dismiss);
		fireEvent.keyDown(screen.getByRole("region", { name: "Question" }), { key: "Escape" });
		expect(answer).not.toHaveBeenCalled();

		act(() => {
			upsert(entities({ links: [mac(DISMISS_SINCE)] }));
		});
		await waitFor(() =>
			expect(screen.getByRole("button", { name: "Dismiss" })).not.toHaveAttribute(
				"aria-disabled"
			)
		);
		fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
		expect(answer).toHaveBeenCalledWith("att-question", DISMISSED, []);
	});

	it("can't be dismissed for a session in a terminal", () => {
		show(question([BRANCH]), { readOnly: true });
		expect(screen.queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();
	});

	it("closes as dismissed, apart from a question closed another way", () => {
		show(
			item(
				{ kind: "question", attention: "att-q", questions: [BRANCH], answers: null },
				"declined"
			)
		);
		expect(screen.getByRole("region", { name: "Dismissed" })).toHaveTextContent(
			`${BRANCH.text}Dismissed`
		);
		cleanup();
		show(
			item(
				{ kind: "question", attention: "att-q", questions: [BRANCH], answers: null },
				"cancelled"
			)
		);
		expect(screen.getByRole("region", { name: "Question closed" })).toBeInTheDocument();
	});

	it("closes an approval Send now skipped as dismissed", () => {
		show({ ...approval(), status: "declined" });
		expect(screen.getByRole("region", { name: "Dismissed" })).toBeInTheDocument();
		cleanup();
		show({ ...approval(), status: "cancelled" });
		expect(screen.getByRole("region", { name: "Request closed" })).toBeInTheDocument();
	});
});

describe("pills in an answer", () => {
	const SIDEBAR = agentOf({ id: "a2", title: "Sidebar sort", vendor: "codex" });
	const SIDEBAR_PILL: AgentRef = {
		type: "agent",
		agent: "a2",
		title: "Sidebar sort",
		vendor: "codex",
		device: DEVICE,
		excerpt: null,
	};

	/** Shows the card for "Fix login" on a Mac whose maxd speaks `protocol`, beside "Sidebar sort". */
	const showOn = (body: TimelineItem, protocol: number) => {
		const here = link({
			id: "mac",
			name: "Mac",
			device: DEVICE,
			route: { type: "local" },
			protocol,
		});
		const { source } = fakeSource({
			agent: heldAgent(),
			links: [here],
			more: entities({ agents: [SIDEBAR], workspaces: [workspace({ id: "w1" })] }),
		});
		return show(body, {}, source);
	};

	/** Types "Ask @side" in `box` and picks "Sidebar sort" from the menu. */
	const nameSidebar = async (box: HTMLElement) => {
		await type(box, "Ask @side");
		fireEvent.click(await screen.findByRole("option", { name: "Sidebar sort" }));
		await waitFor(() =>
			expect(pillsIn(box)).toEqual([expect.stringMatching(/^Agent Sidebar sort/)])
		);
	};

	it("sends an agent named in a question's answer as a pill", async () => {
		const { answer } = showOn(question([OPEN]), ANSWER_PILLS_SINCE);
		await nameSidebar(screen.getByLabelText("Your answer"));
		fireEvent.click(screen.getByRole("button", { name: /Send answer/ }));
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(
				"att-question",
				{ type: "questions", answers: [{ id: "branch", values: [`Ask ${OBJECT_MARK}`] }] },
				[SIDEBAR_PILL]
			)
		);
	});

	it("sends the pill as its words to a maxd too old for pills", async () => {
		const { answer } = showOn(question([OPEN]), ANSWER_PILLS_SINCE - 1);
		await nameSidebar(screen.getByLabelText("Your answer"));
		fireEvent.click(screen.getByRole("button", { name: /Send answer/ }));
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(
				"att-question",
				{
					type: "questions",
					answers: [{ id: "branch", values: ["Ask [Agent: Sidebar sort]"] }],
				},
				[]
			)
		);
	});

	it("sends an agent named in the reason for a deny as a pill", async () => {
		const { answer } = showOn(approval(), ANSWER_PILLS_SINCE);
		await nameSidebar(screen.getByLabelText("Reason for denying"));
		fireEvent.click(screen.getByRole("button", { name: "Deny" }));
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(
				"att-approval",
				{ type: "text", text: `Ask ${OBJECT_MARK}` },
				[SIDEBAR_PILL]
			)
		);
	});

	it("sends a deny's pill as its words to an older maxd", async () => {
		const { answer } = showOn(approval(), ANSWER_PILLS_SINCE - 1);
		await nameSidebar(screen.getByLabelText("Reason for denying"));
		fireEvent.click(screen.getByRole("button", { name: "Deny" }));
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(
				"att-approval",
				{ type: "text", text: "Ask [Agent: Sidebar sort]" },
				[]
			)
		);
	});
});
