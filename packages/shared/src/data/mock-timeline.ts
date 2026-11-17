// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sample timelines for the mock data source. "Usage meter popover" on Claude
// and "HDR tone mapping" on Codex hold every kind of item between them and
// each, "M0 skeleton review" has ten thousand items of history, paged the
// way `maxd` pages them, and "M1 observe milestone" streams a turn on a timer.
// One message in "Usage meter popover" holds every kind of pill.

import type {
	AgentId,
	AttachmentKind,
	AttachmentRef,
	AttentionAnswer,
	AttentionId,
	ClientId,
	FileEdit,
	FileOp,
	FindAt,
	InlineRef,
	ItemBody,
	ItemId,
	ItemStatus,
	TimelineEvent,
	TimelineFind,
	TimelineItem,
	TimelineUpdate,
	Timestamp,
	Todo,
} from "../bindings";
import { collapseSpace, countOccurrences, findQueryOf, selectFind } from "../find/search";
import { OBJECT_MARK } from "../pills/draft";
import { withText } from "../timeline/apply";

import { BINARY_SAMPLE, OUTSIDE_SAMPLE } from "./mock-files";
import { callerCalls } from "./mock-started";
import { CHECKLIST, NOTES_PROJECT } from "./mock-notes";
import { CLAUDE, CODEX, DEVBOX, MAC } from "./mock-view";
import type { CommandResult } from "./source";

const SEC = 1000;
const MIN = 60 * SEC;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

/** The folders the stories run in, as the Mac's workspaces have them. */
const MAXHQ_FOLDER = "/Users/me/Code/Github/Multi/MaxAIManager";
const COMET_FOLDER = "/Users/me/Code/Github/Multi/Comet";

/** The small preview `maxd` made of the popover screenshot, cut out with see-through corners. */
export const POPOVER_PREVIEW =
	"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGAAAAA0CAYAAAB1hz3ZAAAAr0lEQVR42u3bsQ1AQBiGYSMYQWEKlRk0CoMozSI6G5mGCRQicuf+50veUnNPIkhUlZlZ3mua9tTzHPpfMRxYYgSHBSAuwN3F67p/GgQAeQMIQGyAYx5D1XX9qwAAAACgJAB5CgIgAD5F+PQAwC1IAOIB1NuSpLcvRKkCAAAAgJIA5CkIgAAAEAAAAgBAAGIBDMNURAAAuAUBAAAAAAAI/pIEAMDf8g4fRpaHbmb2/S5QQ1eNfeN/8AAAAABJRU5ErkJggg==";

/** How many items a reset or an older page carries. */
export const PAGE = 200;

/** How often the streaming agent moves, about twenty times a second. */
const STREAM_MS = 45;

/** The agent that opens a file for you a moment after you open it. */
export const OPENS_LIVE: AgentId = "agent-open-file";

/** How long after the first open that agent shows its file. */
const OPENS_AFTER_MS = 1500;

/** The file that agent shows. */
const OPENS_PATH = `${MAXHQ_FOLDER}/docs/release-plan.md`;

/** The agent with ten thousand items. */
export const LONG_HISTORY = "agent-m0-review";

/** The agent that streams a turn on a timer. */
export const STREAMING = "agent-m1-lead";

/** The client that answered the sample approvals from MaxHQ. */
export const MOCK_CLIENT: ClientId = "client-devs-macbook";

const OK: CommandResult = { status: "ok", data: null };

type Listener = (update: TimelineUpdate, live: boolean) => void;

// Diffs ---------------------------------------------------------------------

type Hunk = { readonly at: number; readonly lines: readonly string[] };

// A unified diff for one file, with hunk headers and counts worked out from
// lines that start with a space, a plus or a minus.
const fileEdit = (
	path: string,
	op: FileOp,
	hunks: readonly Hunk[],
	old_path: string | null = null
): FileEdit => {
	let additions = 0;
	let deletions = 0;
	let drift = 0;
	const body: string[] = [];
	for (const hunk of hunks) {
		const added = hunk.lines.filter((l) => l.startsWith("+")).length;
		const removed = hunk.lines.filter((l) => l.startsWith("-")).length;
		const context = hunk.lines.length - added - removed;
		const oldStart = op === "add" ? 0 : hunk.at;
		const newStart = op === "delete" ? 0 : hunk.at + drift;
		body.push(`@@ -${oldStart},${context + removed} +${newStart},${context + added} @@`);
		body.push(...hunk.lines);
		drift += added - removed;
		additions += added;
		deletions += removed;
	}
	const before = old_path ?? path;
	const head = [`diff --git a/${before} b/${path}`];
	if (op === "add") head.push("new file mode 100644");
	if (op === "delete") head.push("deleted file mode 100644");
	if (op === "rename") head.push(`rename from ${before}`, `rename to ${path}`);
	head.push(op === "add" ? "--- /dev/null" : `--- a/${before}`);
	head.push(op === "delete" ? "+++ /dev/null" : `+++ b/${path}`);
	const patch = hunks.length > 0 ? [...head, ...body, ""].join("\n") : null;
	return { path, old_path, op, additions, deletions, patch };
};

// Bodies --------------------------------------------------------------------

const attachment = (
	name: string,
	kind: AttachmentKind,
	bytes: number,
	mime: string | null,
	thumbnail: string | null = null
): AttachmentRef => ({
	id: `att-file-${name}`,
	name,
	kind,
	bytes,
	mime,
	path: `/Users/me/Library/Application Support/MaxHQ/staged/${name}`,
	thumbnail,
});

export const you = (
	text: string,
	attachments: AttachmentRef[] = [],
	refs?: InlineRef[]
): ItemBody => ({
	kind: "user_message",
	text,
	attachments,
	...(refs ? { refs } : {}),
	delivery: "sent",
});

export const say = (text: string): ItemBody => ({
	kind: "assistant_message",
	text,
	streaming: false,
});

const sent = (text: string, attachments: AttachmentRef[]): ItemBody => ({
	kind: "agent_attachments",
	text,
	attachments,
});

const think = (text: string): ItemBody => ({ kind: "reasoning", text, streaming: false });

/** A file the agent opened in MaxHQ's viewer for you. */
const opened = (path: string, line: number | null = null): ItemBody => ({
	kind: "agent_opened_file",
	path,
	line,
	outside: false,
});

/** A web page the agent showed you, such as a chart. */
const visual = (title: string, name: string, bytes: number): ItemBody => ({
	kind: "agent_visual",
	title,
	attachment: attachment(name, "file", bytes, "text/html"),
	replaces: null,
});

/** A command you ran with `!` in the composer, ended with `code`. */
const ranByYou = (
	command: string,
	cwd: string,
	output: string,
	code: number,
	took: number
): ItemBody => ({
	kind: "user_run",
	terminal: `0199a7c2-0000-7000-8000-${took.toString(16).padStart(12, "0")}`,
	command,
	cwd,
	device: MAC,
	private: false,
	process: { state: "exited", code, signal: null },
	duration_ms: took,
	lines: output.split("\n").length,
	output,
	output_truncated: false,
	moved: false,
});

const read = (path: string, lines: string | null = null): ItemBody => ({
	kind: "file_read",
	path,
	lines,
});

const search = (query: string, scope: string | null, matches: number | null): ItemBody => ({
	kind: "search",
	query,
	scope,
	matches,
});

const run = (
	command: string,
	output: string,
	exit_code: number | null = 0,
	cwd: string | null = null
): ItemBody => ({
	kind: "command",
	command,
	cwd,
	exit_code,
	output,
	output_truncated: false,
	background: false,
});

const edit = (...changes: FileEdit[]): ItemBody => ({ kind: "file_change", changes });

const todos = (...list: [Todo["state"], string][]): ItemBody => ({
	kind: "todos",
	todos: list.map(([state, text]) => ({ state, text })),
});

const summary = (duration_ms: number, context_tokens: number): ItemBody => ({
	kind: "turn_summary",
	duration_ms,
	context_tokens,
});

// Stories -------------------------------------------------------------------

type Step = {
	/** Time since the item before. */
	readonly gap?: number;
	/** How long the item took. */
	readonly took?: number;
	readonly status?: ItemStatus;
	readonly id?: ItemId;
	readonly parent?: ItemId | null;
};

const OPEN: ReadonlySet<ItemStatus> = new Set(["pending", "running", "waiting"]);

// Writes one agent's items in order on a clock of its own, then moves the
// clock so the last item ends at a given time.
const story = (agent: AgentId) => {
	const items: TimelineItem[] = [];
	let clock = 0;
	let turn = 0;
	const add = (body: ItemBody, step: Step = {}): ItemId => {
		clock += step.gap ?? 3 * SEC;
		const status = step.status ?? "completed";
		const took = step.took ?? 0;
		const id = step.id ?? `${agent}:${items.length}`;
		items.push({
			id,
			agent,
			turn: `${agent}:turn-${turn}`,
			parent: step.parent ?? null,
			ordinal: items.length,
			status,
			started_at: clock,
			completed_at: OPEN.has(status) ? null : clock + took,
			...body,
		});
		clock += took;
		return id;
	};
	return {
		add,
		turn: (pause = 4 * MIN) => {
			turn += 1;
			clock += pause;
		},
		end: (at: Timestamp): TimelineItem[] => {
			const shift = at - clock;
			return items.map((item) => ({
				...item,
				started_at: item.started_at === null ? null : item.started_at + shift,
				completed_at: item.completed_at === null ? null : item.completed_at + shift,
			}));
		},
	};
};

const TEST_OUTPUT = [
	"> maxhq-desktop@0.2.0 test /Users/me/Code/Github/Multi/MaxAIManager/apps/desktop",
	"> vitest run src/header",
	"",
	" RUN  v4.1.2 /Users/me/Code/Github/Multi/MaxAIManager/apps/desktop",
	"",
	" ✓ src/header/usage-rows.test.ts (6 tests) 12ms",
	" ✓ src/header/usage-popover.test.tsx (9 tests) 148ms",
	" ✓ src/header/account-chip.test.tsx (4 tests) 61ms",
	"",
	" Test Files  3 passed (3)",
	"      Tests  19 passed (19)",
	"   Start at  14:41:07",
	"   Duration  1.94s",
	"",
].join("\n");

const POPOVER_REPLY = `The popover now builds its rows from what the account reports, so a plan with only a 5 hour window shows one bar.

| Window | Claude Max | Codex Plus |
| --- | --- | --- |
| 5 hours | 22% used | 100% used, resets 17:10 |
| Week | 45% used | 64% used |

The rows come from one helper, so the sidebar meter and the popover agree:

\`\`\`ts
export const usageRows = (usage: Usage): UsageRow[] =>
	usage.windows
		.filter((w) => w.window_minutes !== null)
		.sort((a, b) => (a.window_minutes ?? 0) - (b.window_minutes ?? 0))
		.map((w) => ({ label: windowLabel(w), percent: w.used_percent, resetsAt: w.resets_at }));
\`\`\`

Here it is on Codex Plus at the limit:

![The popover at the limit](shots/popover-limit.png)

And a recording of it opening:

![Opening the popover](shots/popover-open.mp4)

Two things to check by hand:

1. Open the popover on **Codex Plus** while it's at the limit. The 5 hour row turns amber.
2. Switch the sample data to *devbox offline* and confirm the popover still opens.

The tests in \`src/header\` pass. I didn't touch the Rust side.`;

const POPOVER_PLAN = `## Hide usage rows the vendor doesn't report

1. Add \`usageRows\` in \`src/header/usage-rows.ts\` that keeps only the windows the account reports.
2. Build the popover rows from it, so the weekly bar goes away for plans without one.
3. Turn the 5 hour row amber at 90% and red at the limit, as the mockup shows.
4. Cover Claude Max and Codex Plus in \`usage-rows.test.ts\`.

Nothing changes in \`maxd\`. The protocol already sends each window with its length.`;

const M = OBJECT_MARK;

const QUOTED = "a plan with only a 5 hour window shows one bar";

/**
 * A message with each kind of pill: a picture and a PDF, a quote of the
 * reply, an agent, one that's gone with its excerpt, a pull request and an
 * issue.
 */
const popoverFollowUp = (reply: ItemId, now: Timestamp): ItemBody => {
	const at = POPOVER_REPLY.indexOf(QUOTED);
	const meter = attachment("sidebar-meter.png", "image", 61_240, "image/png", POPOVER_PREVIEW);
	const spec = attachment("meter-spec.pdf", "pdf", 96_400, "application/pdf");
	const excerpt = attachment("agent-meter-colors.md", "file", 5_820, "text/markdown");
	// A note from another project goes with a copy.
	const checklist = attachment("GTFS release steps.md", "file", 160, "text/markdown");
	const git = (number: number, kind: "pull_request" | "issue", title: string): InlineRef => ({
		type: "git_item",
		repo: "Example-User/MaxHQ",
		number,
		kind,
		title,
		state: "open",
		url: `https://github.com/Example-User/MaxHQ/${kind === "pull_request" ? "pull" : "issues"}/${number}`,
		head: kind === "pull_request" ? "usage-cache" : null,
		base: kind === "pull_request" ? "main" : null,
		body: "",
		checked_at: now - 6 * MIN,
	});
	return you(
		`Also check the sidebar meter ${M} against ${M}, so it matches what you said: ${M}. ${M} is changing the sidebar, and ${M} picked the colors, so leave them. Follow ${M} when it ships. This goes in #412 and fixes #405, then commit.`,
		[meter, spec, excerpt, checklist],
		[
			{ type: "attachment", id: meter.id },
			{ type: "attachment", id: spec.id },
			{
				type: "quote",
				agent: "agent-usage-popover",
				agent_title: "Usage meter popover",
				item: reply,
				text: QUOTED,
				position: POPOVER_REPLY.slice(0, at).replace(/\s+/g, " ").length,
				source: "reply",
				from: null,
			},
			{
				type: "agent",
				agent: "agent-sidebar-sort",
				title: "Sidebar sort order",
				vendor: "claude",
				device: MAC,
				excerpt: null,
			},
			// An agent removed since, so its pill shows as gone.
			{
				type: "agent",
				agent: "agent-meter-colors",
				title: "Meter colors",
				vendor: "codex",
				device: DEVBOX,
				excerpt: excerpt.id,
			},
			{
				type: "note",
				project: NOTES_PROJECT,
				note: CHECKLIST,
				title: "GTFS release steps",
				copy: checklist.id,
			},
			git(412, "pull_request", "Cache usage between polls"),
			git(405, "issue", "Usage meter shows the wrong reset time"),
		]
	);
};

// "Usage meter popover", on Claude Max. Every kind of item is here.
const usagePopover = (now: Timestamp): TimelineItem[] => {
	const s = story("agent-usage-popover");
	s.add(
		you(
			"The usage popover shows a weekly bar even when an account has no weekly window. Hide the rows the vendor doesn't report, and match the mockup.",
			[
				attachment("popover-today.png", "image", 184_220, "image/png", POPOVER_PREVIEW),
				attachment("usage-popover-mockup.pdf", "pdf", 412_900, "application/pdf"),
			]
		)
	);
	s.add(
		think(
			"The popover lists a fixed pair of rows, 5 hours and Week. The account's usage already carries `window_minutes` for each window, so the rows should come from that list. The sidebar meter has its own copy of the labels, which would drift. One helper for both."
		),
		{ took: 12 * SEC }
	);
	s.add(read("apps/desktop/src/header/usage-popover.tsx", "1-92"));
	s.add(read("apps/desktop/src/components/status/usage.ts"), { gap: SEC });
	s.add(read("docs/usage-popover.md"), { gap: SEC });
	s.add(read("design/usage-popover.png"), { gap: SEC });
	s.add(read("design/usage-popover-mockup.pdf"), { gap: SEC });
	s.add(read("design/meter-stand.glb"), { gap: SEC });
	s.add(read("design/usage-samples.csv"), { gap: SEC });
	s.add(read("crates/usage/src/windows.rs", "24-34"), { gap: SEC });
	s.add(read("apps/desktop/src/header/usage-rows.ts"), { gap: SEC });
	s.add(read(BINARY_SAMPLE), { gap: SEC });
	s.add(read(OUTSIDE_SAMPLE), { gap: SEC });
	s.add(search("window_minutes", "apps/desktop/src", 7), { gap: SEC });
	s.add(search("weeklyRow", null, 0), { gap: SEC });
	s.add(
		{
			kind: "subagent",
			subagent: null,
			agent_type: "Explore",
			description: "find usage call sites",
			result: "Four places read usage windows: the header chip, the popover, the sidebar meter and the limit wait card. Only the popover assumes a weekly window.",
		},
		{ took: 38 * SEC }
	);
	s.add({ kind: "web", action: "search", target: "Claude Max weekly usage limit reset time" });
	s.add({
		kind: "web",
		action: "fetch",
		target: "https://support.anthropic.com/en/articles/usage-limit-best-practices",
	});
	s.add({
		kind: "tool_call",
		server: "context7",
		name: "get-library-docs",
		title: "Base UI popover, positioning and focus",
		input: '{ "library": "/mui/base-ui", "topic": "popover" }',
		output: "Popover.Root, Popover.Trigger, Popover.Portal, Popover.Positioner, Popover.Popup. The positioner takes side, align and sideOffset.",
	});
	s.add({
		kind: "tool_call",
		server: "browser",
		name: "screenshot",
		title: "The popover as it is now",
		input: '{ "selector": "[data-usage-popover]" }',
		output: "Took a screenshot of the popover.",
		images: [
			{
				...attachment("image-3f9a2c1b.png", "image", 48_213, "image/png", POPOVER_PREVIEW),
				id: "att-tool-popover",
				path: "/Users/me/Library/Application Support/MaxHQ/attachments/agent-usage-popover/tool-output/3f9a2c1b.png",
			},
		],
	});
	s.add({
		kind: "question",
		attention: "att-popover-rows",
		questions: [
			{
				id: "missing",
				header: "Missing windows",
				text: "When an account doesn't report a window, should its row disappear or say it isn't reported?",
				options: [
					{
						label: "Hide the row",
						description: "The popover only shows what the vendor sends.",
					},
					{
						label: "Show it as not reported",
						description: "Rows stay in the same place for every plan.",
					},
				],
				multi: false,
				other: true,
				secret: false,
			},
		],
		answers: [{ id: "missing", values: ["Hide the row"] }],
	});
	s.add({ kind: "plan", markdown: POPOVER_PLAN, streaming: false }, { took: 9 * SEC });
	s.add({
		kind: "approval",
		attention: "att-popover-plan",
		approval_kind: "plan",
		title: "Ready to code?",
		detail: null,
		cwd: MAXHQ_FOLDER,
		options: [
			{ id: "accept_edits", label: "Yes, and accept edits" },
			{ id: "accept", label: "Yes, and ask before each edit" },
			{ id: "deny", label: "No, keep planning" },
		],
		decided: {
			answer: { type: "option", id: "accept" },
			by: MOCK_CLIENT,
			at: 0,
		},
	});
	s.add(
		todos(
			["done", "Add usageRows with the windows the account reports"],
			["done", "Build the popover rows from usageRows"],
			["done", "Color the 5 hour row at 90% and at the limit"],
			["in_progress", "Cover Claude Max and Codex Plus in tests"],
			["pending", "Check the sidebar meter uses the same labels"]
		)
	);
	s.add(
		edit(
			fileEdit("apps/desktop/src/header/usage-popover.tsx", "modify", [
				{
					at: 41,
					lines: [
						" \tconst usage = account.usage;",
						" \tif (!usage) return <NotReported account={account} />;",
						"-\tconst rows = [fiveHourRow(usage), weeklyRow(usage)];",
						"+\tconst rows = usageRows(usage);",
						"+\tconst limited = rows.some((row) => row.percent >= 100);",
						"+\tconst near = !limited && rows.some((row) => row.percent >= 90);",
						" \treturn (",
						' \t\t<PopoverPopup className={cn(limited && "border-status-busy/40")}>',
					],
				},
			]),
			fileEdit("apps/desktop/src/header/usage-rows.ts", "add", [
				{
					at: 1,
					lines: [
						'+import type { Usage } from "@/lib/bindings";',
						'+import { windowLabel } from "@/components/status/usage";',
						"+",
						"+export const usageRows = (usage: Usage) =>",
						"+\tusage.windows",
						"+\t\t.filter((w) => w.window_minutes !== null)",
						"+\t\t.sort((a, b) => (a.window_minutes ?? 0) - (b.window_minutes ?? 0))",
						"+\t\t.map((w) => ({ label: windowLabel(w), percent: w.used_percent }));",
						"+",
					],
				},
			])
		),
		{ took: 2 * SEC }
	);
	s.add({
		kind: "approval",
		attention: "att-popover-test",
		approval_kind: "command",
		title: "Run `pnpm test:desktop -- src/header`?",
		detail: "pnpm test:desktop -- src/header",
		cwd: MAXHQ_FOLDER,
		options: [
			{ id: "allow", label: "Allow once" },
			{ id: "session", label: "Allow for this session" },
			{ id: "deny", label: "Deny" },
		],
		decided: { answer: { type: "option", id: "session" }, by: MOCK_CLIENT, at: 0 },
	});
	s.add(run("pnpm test:desktop -- src/header", TEST_OUTPUT), { took: 7100 });
	s.add({
		kind: "error",
		message: "The API is overloaded. Claude retries in 8 s, attempt 2 of 10.",
		retrying: true,
	});
	s.add({ kind: "limit_wait", account: CLAUDE, resets_at: now - 52 * MIN, auto_resume: true });
	s.add({ kind: "compaction", before_tokens: 168_000, after_tokens: 41_000 }, { took: 14 * SEC });
	s.add({
		kind: "notice",
		level: "info",
		message: "Claude Code updated to 2.3.1. The next turn uses the new version.",
	});
	s.add({
		kind: "notice",
		level: "warning",
		message: "A PreToolUse hook in .claude/settings.json took 4 s to run.",
	});
	s.add({ kind: "unknown", native_kind: "system:hook_progress" });
	s.add(
		sent("The popover with both windows, as it draws now.", [
			attachment("popover-after.png", "image", 176_480, "image/png", POPOVER_PREVIEW),
		])
	);
	const reply = s.add(say(POPOVER_REPLY), { took: 6 * SEC });
	s.add(summary(4 * MIN + 12 * SEC, 81_000));
	// You moved it to Codex, then back before sending it anything.
	s.add(
		{
			kind: "handoff",
			from_vendor: "claude",
			from_model: null,
			to_vendor: "codex",
			to_model: "gpt-5.5",
		},
		{ gap: MIN, status: "cancelled" }
	);
	s.add(
		{
			kind: "handoff",
			from_vendor: "codex",
			from_model: "gpt-5.5",
			to_vendor: "claude",
			to_model: null,
		},
		{ gap: 20 * SEC }
	);

	s.turn();
	s.add(popoverFollowUp(reply, now));
	s.add(read("apps/desktop/src/sidebar/usage-meter.tsx", "1-64"));
	s.add(
		{
			kind: "approval",
			attention: "att-popover-edit",
			approval_kind: "file_change",
			title: "Edit apps/desktop/src/sidebar/usage-meter.tsx?",
			detail: null,
			cwd: MAXHQ_FOLDER,
			options: [
				{ id: "allow", label: "Allow once" },
				{ id: "session", label: "Allow for this session" },
				{ id: "deny", label: "Deny" },
			],
			decided: {
				answer: {
					type: "text",
					text: "Leave the sidebar alone for now, I'm changing it in another branch.",
				},
				by: null,
				at: 0,
			},
		},
		{ status: "declined" }
	);
	s.add(
		edit(
			fileEdit("apps/desktop/src/sidebar/usage-meter.tsx", "modify", [
				{
					at: 12,
					lines: [
						' import { cn } from "@/lib/utils";',
						'-import { fiveHourRow, weeklyRow } from "@/header/usage-popover";',
						'+import { usageRows } from "@/header/usage-rows";',
					],
				},
			])
		),
		{ status: "declined" }
	);
	s.add(
		{
			kind: "command",
			command: 'git commit -am "Hide usage rows the vendor doesn\'t report"',
			cwd: MAXHQ_FOLDER,
			exit_code: null,
			output: "",
			output_truncated: false,
			background: false,
		},
		{ status: "interrupted", took: 2 * SEC }
	);
	s.add({ kind: "interrupted" });
	s.add(summary(48 * SEC, 87_000));
	s.turn(MIN);
	s.add(you("Show me the popover notes."));
	s.add(opened(`${MAXHQ_FOLDER}/docs/usage-popover.md`, 12));
	s.add(say("The popover notes are open in the viewer at line 12."));
	s.add(ranByYou("git status --short", MAXHQ_FOLDER, " M src/usage/popover.tsx", 0, 140), {
		took: 140,
	});
	s.turn(MIN);
	s.add(you("How did usage move this week?"));
	s.add(visual("Weekly usage by vendor", "visual-weekly-usage-by-vendor.html", 6_240));
	s.add(say("Claude climbed through Thursday, then eased off once the limit reset."));
	return s.end(now - 4 * MIN);
};

const HDR_REASONING = `**Finding where mpv gets its options**

The player sets \`vo=gpu\` when the plugin creates the render context, so the change belongs there. Tone mapping needs \`gpu-next\`, and the setting has to reach the plugin from the settings store.`;

const SCP_OUTPUT = "hdr10-sample.mkv                              100%  412MB  38.2MB/s   00:10\n";

const CARGO_OUTPUT = [
	"   Compiling tauri-plugin-mpv v0.4.0 (/Users/me/Code/Github/Multi/Comet/crates/tauri-plugin-mpv)",
	"    Finished `test` profile [unoptimized + debuginfo] target(s) in 18.42s",
	"     Running unittests src/lib.rs (target/debug/deps/tauri_plugin_mpv-5c1d0a9e2f)",
	"",
	"running 4 tests",
	"test render::tests::tone_mapping_defaults_to_auto ... ok",
	"test render::tests::clip_turns_tone_mapping_off ... ok",
	"test render::tests::hdr_sample_peaks_below_1000_nits ... FAILED",
	"test settings::tests::playback_section_has_tone_mapping ... ok",
	"",
	"failures:",
	"",
	"---- render::tests::hdr_sample_peaks_below_1000_nits stdout ----",
	"thread 'render::tests::hdr_sample_peaks_below_1000_nits' panicked at crates/tauri-plugin-mpv/src/render.rs:212:9:",
	"assertion failed: peak < 1000.0",
	"",
	"test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.84s",
	"",
].join("\n");

// "HDR tone mapping", on Codex Plus. Every kind of item, ending at the limit.
const hdrToneMapping = (now: Timestamp): TimelineItem[] => {
	const s = story("agent-hdr");
	s.add(
		you("HDR films look washed out on the living room TV. Is that mpv or the TV?", [
			attachment("player.log", "file", 220_400, "text/plain"),
		])
	);
	s.add(
		say(
			"It's the player. The log shows a PQ source going to an SDR output with tone mapping off, so the highlights get clipped. mpv can map it. I'd add a setting with auto, clip and off. Should auto be the default?"
		),
		{ took: 4 * SEC }
	);
	// The question went to Claude, and the work to Codex.
	s.add(
		{
			kind: "handoff",
			from_vendor: "claude",
			from_model: "sonnet",
			to_vendor: "codex",
			to_model: null,
		},
		{ gap: 5 * MIN }
	);
	s.turn(MIN);
	s.add(you("Yes, auto by default. Put it under Playback."));
	s.add(think(HDR_REASONING), { took: 9 * SEC });
	s.add(read("crates/tauri-plugin-mpv/src/render.rs", "1-214"));
	s.add(search("tone-mapping", "crates src", 0), { gap: SEC });
	s.add(
		{ kind: "web", action: "search", target: "mpv gpu-next tone-mapping auto" },
		{ gap: SEC }
	);
	s.add({
		kind: "web",
		action: "fetch",
		target: "https://mpv.io/manual/stable/#options-tone-mapping",
	});
	s.add({
		kind: "tool_call",
		server: "github",
		name: "search_issues",
		title: "mpv issues about HDR on macOS",
		input: '{ "repo": "mpv-player/mpv", "query": "tone-mapping macOS gpu-next" }',
		output: "12 issues. The newest says target-colorspace-hint needs macOS 14 or later.",
	});
	s.add({
		kind: "question",
		attention: "att-hdr-modes",
		questions: [
			{
				id: "modes",
				header: "Modes",
				text: "Which modes should the setting offer?",
				options: [
					{ label: "Auto", description: "mpv picks from the source and the display." },
					{ label: "Clip", description: "Cut highlights, the current look." },
					{ label: "Off", description: "Pass the signal through untouched." },
					{ label: "BT.2390", description: "Fixed curve, for testing." },
				],
				multi: true,
				other: false,
				secret: false,
			},
		],
		answers: [{ id: "modes", values: ["Auto", "Clip", "Off"] }],
	});
	s.add(
		{
			kind: "plan",
			markdown:
				"1. Switch the render context to `gpu-next` and pass the tone mapping mode.\n2. Add a Tone mapping select under Playback, defaulting to auto.\n3. Test with a 10-bit HDR sample from devbox.",
			streaming: false,
		},
		{ took: 5 * SEC }
	);
	s.add(
		todos(
			["done", "Use gpu-next and pass the tone mapping mode"],
			["done", "Add the Tone mapping setting under Playback"],
			["in_progress", "Check a 10-bit HDR sample"]
		)
	);
	s.add(
		{
			kind: "subagent",
			subagent: null,
			agent_type: "worker",
			description: "check which mpv build the CI uses",
			result: "CI builds against libmpv 0.39 from Homebrew, which has gpu-next.",
		},
		{ took: 51 * SEC }
	);
	s.add(
		edit(
			fileEdit("crates/tauri-plugin-mpv/src/render.rs", "modify", [
				{
					at: 56,
					lines: [
						" fn configure(mpv: &Mpv, opts: &RenderOptions) -> Result<()> {",
						' \tmpv.set_property("hwdec", "auto-safe")?;',
						'-\tmpv.set_property("vo", "gpu")?;',
						'+\tmpv.set_property("vo", "gpu-next")?;',
						'+\tmpv.set_property("target-colorspace-hint", "yes")?;',
						'+\tmpv.set_property("tone-mapping", opts.tone_mapping.as_str())?;',
						'+\tmpv.set_property("hdr-compute-peak", "auto")?;',
						" \tOk(())",
						" }",
					],
				},
			]),
			fileEdit("src/lib/settings.ts", "modify", [
				{
					at: 30,
					lines: [
						" \tplayback: {",
						" \t\tvolume: 100,",
						'+\t\ttoneMapping: "auto" as ToneMapping,',
						" \t},",
						" };",
						"+",
						'+export type ToneMapping = "auto" | "clip" | "off";',
						"+",
						'+export const TONE_MAPPING: readonly ToneMapping[] = ["auto", "clip", "off"];',
						"+",
						"+export const toneMappingLabel = (mode: ToneMapping) =>",
						'+\tmode === "auto" ? "Auto" : mode === "clip" ? "Clip" : "Off";',
					],
				},
			])
		),
		{ took: 3 * SEC }
	);
	s.add(
		say(
			"Tone mapping is now on, with `auto` as the default. To check the result I need a 10-bit HDR sample. There's one on devbox, but the sandbox has no network access, so I have to ask."
		)
	);
	s.add(
		{
			kind: "approval",
			attention: "att-hdr-scp",
			approval_kind: "network",
			title: "Run `scp` with network access?",
			detail: "scp devbox:/srv/media/samples/hdr10-sample.mkv tests/fixtures/",
			cwd: COMET_FOLDER,
			options: [
				{ id: "accept", label: "Approve" },
				{ id: "accept_for_session", label: "Approve for session" },
				{ id: "decline", label: "Decline" },
			],
			decided: { answer: { type: "option", id: "accept" }, by: MOCK_CLIENT, at: 0 },
		},
		{ took: 2 * MIN }
	);
	s.add(run("scp devbox:/srv/media/samples/hdr10-sample.mkv tests/fixtures/", SCP_OUTPUT), {
		took: 10_400,
	});
	s.add({
		kind: "error",
		message: "stream disconnected before completion. Reconnecting, 1 of 5.",
		retrying: true,
	});
	s.add(run("cargo test -p tauri-plugin-mpv", CARGO_OUTPUT, 101), {
		took: 21_300,
		status: "failed",
	});
	s.add(
		sent("The whole test log, if you want more than the failure.", [
			attachment("mpv-test.log", "file", 48_300, "text/plain"),
		])
	);
	s.add({ kind: "compaction", before_tokens: 241_000, after_tokens: 58_000 }, { took: 11 * SEC });
	s.add({
		kind: "notice",
		level: "warning",
		message: "The sandbox blocks network access. Commands that need it ask first.",
	});
	s.add({ kind: "unknown", native_kind: "mcp_startup_update" });
	s.add({ kind: "interrupted" });
	s.add(opened(`${COMET_FOLDER}/docs/hdr-tone-mapping.md`));
	s.add(visual("Tone curve by mode", "visual-tone-curve-by-mode.html", 8_120));
	s.add(ranByYou("cargo fmt --check", COMET_FOLDER, "Diff in src/tone.rs at line 88", 1, 1_900), {
		took: 1_900,
		status: "failed",
	});
	s.add(summary(7 * MIN + 4 * SEC, 151_000));
	s.turn(MIN);
	s.add(you("Keep going, the peak test is fine to loosen for this sample."));
	s.add(read("crates/tauri-plugin-mpv/src/render.rs", "200-214"));
	s.add(
		{
			kind: "limit_wait",
			account: CODEX,
			resets_at: now + 2 * HOUR + 22 * MIN,
			auto_resume: true,
		},
		{ status: "running" }
	);
	// Codex picks up where it stopped once the limit resets, because this waits.
	s.add(
		{
			...you("Then add the Tone mapping option to the Playback settings screen."),
			delivery: "queued",
		} as ItemBody,
		{ status: "pending" }
	);
	return s.end(now - 18 * MIN);
};

// "Release checklist", waiting on a command approval with a message queued behind it.
const releaseChecklist = (now: Timestamp): TimelineItem[] => {
	const s = story("agent-release");
	s.add(you("Go through the release checklist for 0.3.0 on Dev's MacBook."));
	s.add(
		think("The checklist lives in RELEASING.md. Version bumps first, then the full test run."),
		{
			took: 4 * SEC,
		}
	);
	s.add(read("RELEASING.md"));
	s.add(read("apps/desktop/src-tauri/tauri.conf.json", "1-40"));
	s.add(search('"version"', "Cargo.toml apps/desktop/package.json", 3));
	s.add(
		todos(
			["done", "Check the version in every manifest"],
			["in_progress", "Run the full test suite"],
			["pending", "Build a signed bundle"],
			["pending", "Write the release notes"]
		)
	);
	s.add(say("Every manifest says 0.2.0, so nothing is out of step. Next is the full test run."));
	// It hands the notes and the bundle to agents of their own.
	for (const body of callerCalls()) s.add(body, { took: 2 * SEC });
	// The command waits on the approval that sits under it.
	const tests = s.add(run("cargo test --workspace", "", null, MAXHQ_FOLDER), {
		status: "pending",
	});
	s.add(
		{
			kind: "approval",
			attention: "att-release",
			approval_kind: "command",
			title: "Run `cargo test --workspace`?",
			detail: "cargo test --workspace",
			cwd: MAXHQ_FOLDER,
			options: [
				{ id: "allow", label: "Allow once" },
				{ id: "session", label: "Allow for this session" },
				{ id: "deny", label: "Deny" },
			],
			decided: null,
		},
		{ status: "waiting", parent: tests, gap: 0 }
	);
	s.add(
		{
			...you("Once the tests pass, bump everything to 0.3.0."),
			delivery: "queued",
		} as ItemBody,
		{ status: "pending" }
	);
	return s.end(now - 2 * MIN);
};

// "Subtitle sync drift", on devbox, waiting on a question.
const syncDrift = (now: Timestamp): TimelineItem[] => {
	const s = story("agent-sync-drift");
	s.add(
		you("Subtitles drift by about a second over a whole episode of The Expanse. Find out why.")
	);
	s.add(think("A drift that grows over time means a frame rate mismatch, not an offset."), {
		took: 7 * SEC,
	});
	s.add(read("src/player/subtitles/sync.ts", "1-120"));
	s.add(search("frameRate", "src/player", 11));
	s.add(
		run(
			"ffprobe -v error -show_streams media/expanse-s01e01.mkv | grep r_frame_rate",
			"r_frame_rate=24000/1001\nr_frame_rate=0/0\n"
		),
		{ took: 900 }
	);
	s.add(
		say(
			"The video runs at 23.976 frames a second and the subtitle file was timed for 25, which adds up to about a second of drift by the end. I can fix it with a scale factor plus an offset."
		)
	);
	s.add(
		{
			kind: "question",
			attention: "att-sync-drift",
			questions: [
				{
					id: "scope",
					header: "Sync offset",
					text: "Should the sync offset apply per file or per show?",
					options: [
						{ label: "Per file", description: "Each episode keeps its own offset." },
						{
							label: "Per show",
							description: "One offset for every episode of the show.",
						},
					],
					multi: false,
					other: true,
					secret: false,
				},
			],
			answers: null,
		},
		{ status: "waiting" }
	);
	return s.end(now - 6 * MIN);
};

// Ten thousand items ---------------------------------------------------------

const FILES = [
	"crates/daemon/src/main.rs",
	"crates/daemon/src/replica.rs",
	"crates/daemon/src/observe/claude.rs",
	"crates/daemon/src/observe/codex.rs",
	"crates/protocol/src/lib.rs",
	"crates/protocol/src/timeline.rs",
	"apps/desktop/src/App.tsx",
	"apps/desktop/src/view/store.ts",
	"apps/desktop/src/sidebar/agent-row.tsx",
	"apps/desktop/src-tauri/src/lib.rs",
	"scripts/check-licenses.mjs",
	"Cargo.toml",
];

const ASKS = [
	"Read through the M0 skeleton and list what's missing before M1.",
	"Does the replica handle a device that comes back with a new boot ID?",
	"Check the protocol crate for anything that isn't versioned.",
	"Look at how the sidebar sorts agents and whether it's stable.",
	"Are there tests for the Unix socket path on macOS?",
	"Review the license script against the dependency list.",
];

const REPLIES = [
	"The skeleton holds up. The daemon starts, the replica sends a snapshot and deltas, and the desktop app renders the sample view. What's missing is mostly error paths.",
	"It does. A new boot ID marks every agent on the device as gone, then the observer rebuilds them from the session files.",
	"Everything on the wire carries the protocol version in the hello. Two enums lack an unknown variant, which would break older clients.",
	"The sort is stable within a project. Across projects it falls back to the title, which reorders rows when a title changes.",
	"There's one, and it only runs on Linux in CI. The path length limit on macOS isn't covered.",
	"Every package passes. Two declare their license in lowercase, which the script accepts.",
];

const pick = <T>(list: readonly T[], n: number): T => list[n % list.length] as T;

// One item of the long history, the same every time for the same index.
const historyBody = (n: number): ItemBody => {
	const turn = Math.floor(n / 20);
	const step = n % 20;
	const file = pick(FILES, n * 7 + turn);
	switch (step) {
		case 0:
			return you(`Round ${turn + 1}. ${pick(ASKS, turn)}`);
		case 1:
			return think(
				`Start from ${file}, since that's where ${pick(["the state", "the wire format", "the sort", "the socket"], turn)} is defined. Then follow the callers.`
			);
		case 2:
		case 3:
		case 6:
		case 11:
		case 14:
			return read(file, step % 2 === 0 ? `${(n % 90) + 1}-${(n % 90) + 60}` : null);
		case 4:
		case 12:
			return search(
				pick(["BootId", "Delta", "sort_key", "socket_path", "allowlist"], n),
				"crates",
				n % 9
			);
		case 5:
		case 15:
			return say(`${pick(REPLIES, turn + step)} Item ${n}.`);
		case 7:
			return edit(
				fileEdit(file, "modify", [
					{
						at: (n % 200) + 10,
						lines: [
							` // Step ${n}`,
							`-let value = old_value_${n};`,
							`+let value = new_value_${n};`,
							` return value;`,
						],
					},
				])
			);
		case 8:
		case 16:
			return run(
				pick(
					["cargo test -p maxhq-daemon", "pnpm test:desktop", "cargo clippy --workspace"],
					n
				),
				`running ${n % 40} tests\n${"test ok\n".repeat((n % 6) + 1)}test result: ok. ${n % 40} passed; 0 failed\n`
			);
		case 9:
			return say(
				`Here is what I found in \`${file}\`:\n\n- ${pick(REPLIES, turn)}\n- The callers are covered by tests.\n\n\`\`\`rust\nfn check_${n}() -> bool {\n    true\n}\n\`\`\``
			);
		case 10:
			return n % 1000 === 10
				? { kind: "compaction", before_tokens: 180_000, after_tokens: 40_000 }
				: { kind: "web", action: "search", target: `tokio unix socket ${turn}` };
		case 13:
			return {
				kind: "tool_call",
				server: null,
				name: "Glob",
				title: `${pick(["crates/**/*.rs", "apps/desktop/src/**/*.tsx"], n)}`,
				input: null,
				output: `${n % 30} files`,
			};
		case 17:
			return n % 500 === 17
				? {
						kind: "notice",
						level: "info",
						message: `Claude Code updated to 2.${turn % 9}.0.`,
					}
				: read(file);
		case 18:
			return say(pick(REPLIES, turn + 3));
		default:
			return summary(((n % 7) + 1) * 41 * SEC, 30_000 + (n % 170) * 1000);
	}
};

const longHistory = (now: Timestamp, count: number): TimelineItem[] => {
	const end = now - 3 * DAY;
	const items: TimelineItem[] = [];
	for (let n = 0; n < count; n++) {
		const at = end - (count - n) * 17 * SEC;
		items.push({
			id: `${LONG_HISTORY}:${n}`,
			agent: LONG_HISTORY,
			turn: `${LONG_HISTORY}:turn-${Math.floor(n / 20)}`,
			parent: null,
			ordinal: n,
			status: "completed",
			started_at: at,
			completed_at: at + (n % 20 === 1 ? 6 * SEC : n % 20 === 8 ? 3200 : 0),
			...historyBody(n),
		});
	}
	return items;
};

// Short timelines for everyone else ------------------------------------------

type Brief = {
	readonly ask: string;
	readonly file: string;
	readonly command: string;
	readonly reply: string;
};

const BRIEFS: Readonly<Record<AgentId, Brief>> = {
	"agent-w1": {
		ask: "Build the maxd core: config, the socket and the replica.",
		file: "crates/daemon/src/replica.rs",
		command: "cargo test -p maxhq-daemon replica",
		reply: "The replica sends a snapshot on connect and deltas after it. Tests cover reconnects.",
	},
	"agent-w2": {
		ask: "Observe Claude Code sessions from their session files.",
		file: "crates/daemon/src/observe/claude.rs",
		command: "cargo test -p maxhq-daemon observe::claude",
		reply: "The observer tails each session file and maps its entries to agents.",
	},
	"agent-w3": {
		ask: "Observe Codex sessions from the rollout files.",
		file: "crates/daemon/src/observe/codex.rs",
		command: "cargo test -p maxhq-daemon observe::codex",
		reply: "Rollouts map to agents, and rate limit events update the Codex Plus account.",
	},
	"agent-w4": {
		ask: "Write the client crate that talks to maxd.",
		file: "crates/client/src/lib.rs",
		command: "cargo test -p maxhq-client",
		reply: "The client connects over the socket or ssh and keeps a replica of the view.",
	},
	"agent-w5": {
		ask: "Wire the desktop backend to the client crate.",
		file: "apps/desktop/src-tauri/src/lib.rs",
		command: "cargo build -p maxhq-desktop",
		reply: "The backend forwards deltas to the webview and exposes the commands.",
	},
	"agent-w6": {
		ask: "Build the sidebar and header on the sample data.",
		file: "apps/desktop/src/sidebar/agent-row.tsx",
		command: "pnpm test:desktop",
		reply: "The sidebar groups agents by device and project, with a dot for each state.",
	},
	"agent-w7": {
		ask: "Observe agents inside the Incus containers on devbox.",
		file: "crates/daemon/src/observe/incus.rs",
		command: "cargo test -p maxhq-daemon observe::incus",
		reply: "Each container on devbox reports its agents through incus exec.",
	},
	"agent-sidebar-sort": {
		ask: "Sort the sidebar so agents that need you come first.",
		file: "apps/desktop/src/sidebar/sort.ts",
		command: "pnpm test:desktop -- sidebar",
		reply: "Agents that need input sort first, then busy ones, then the rest by last activity.",
	},
	"agent-bindings": {
		ask: "Check that bindings.ts matches the protocol crate.",
		file: "apps/desktop/src/lib/bindings.ts",
		command: "cargo test -p maxhq-desktop export_bindings",
		reply: "The generated bindings match. Nothing to commit.",
	},
	"agent-license-check": {
		ask: "Make the license check fail on anything outside the allowlist.",
		file: "scripts/check-licenses.mjs",
		command: "node scripts/check-licenses.mjs",
		reply: "The script now exits with status 1 and names each package outside the allowlist.",
	},
	"agent-font-fallback": {
		ask: "Japanese subtitles render as boxes. Add a font fallback.",
		file: "src/player/subtitles/fonts.ts",
		command: "pnpm test -- fonts",
		reply: "Subtitles now fall back to Hiragino Sans on macOS and Noto Sans CJK on Linux.",
	},
	"agent-gtfs": {
		ask: "Refresh the GTFS feed and rebuild the stop index.",
		file: "src/feed/gtfs.ts",
		command: "pnpm feed:refresh",
		reply: "The feed has 4,812 stops this week, 9 more than last week.",
	},
	"agent-stop-times": {
		ask: "Cache stop times so the departures board loads offline.",
		file: "src/cache/stop-times.ts",
		command: "pnpm test -- stop-times",
		reply: "Stop times for the next six hours are cached on each refresh.",
	},
	"agent-df-chaos": {
		ask: "Add a chaos block that fills the disk and checks the agent recovers.",
		file: "chaos/df.sh",
		command: "./chaos/df.sh --dry-run",
		reply: "The block fills /tmp to 95% and waits for the agent to report the error.",
	},
	"agent-reboot-bracket": {
		ask: "Bracket workspace reboots so agents resume after the container restarts.",
		file: "workspaces/reboot.sh",
		command: "incus restart ws-charlie",
		reply: "The container restarts and the agent resumes from its session file.",
	},
	"agent-egress": {
		ask: "Audit the egress allowlist on devbox.",
		file: "infra/devbox/egress.nft",
		command: "nft list ruleset",
		reply: "Three hosts on the allowlist are no longer used.",
	},
	"agent-lesson-audio": {
		ask: "Generate audio for the new lessons.",
		file: "pipeline/audio.py",
		command: "uv run pipeline/audio.py --lesson 14",
		reply: "Lesson 14 has audio for all 32 phrases.",
	},
};

const brief = (agent: AgentId, now: Timestamp): TimelineItem[] => {
	const b = BRIEFS[agent] ?? {
		ask: "Pick up where the last session left off.",
		file: "README.md",
		command: "git status",
		reply: "Everything is committed.",
	};
	const s = story(agent);
	s.add(you(b.ask));
	s.add(think(`Start with ${b.file} and see what's there.`), { took: 5 * SEC });
	s.add(read(b.file));
	s.add(run(b.command, "ok\n"), { took: 4200 });
	s.add(say(b.reply), { took: 3 * SEC });
	s.add(summary(MIN + 20 * SEC, 52_000));
	return s.end(now - 2 * MIN);
};

/** The lead's sub-agents that are still at work, each partway into its turn. */
const WORKING: ReadonlySet<AgentId> = new Set([
	"agent-w1",
	"agent-w2",
	"agent-w3",
	"agent-w5",
	"agent-w6",
	"agent-w7",
]);

// A sub-agent mid-turn: a few reads and an edit behind it, its tests running now.
const worker = (agent: AgentId, now: Timestamp): TimelineItem[] => {
	const b = BRIEFS[agent]!;
	const folder = b.file.split("/").slice(0, -1).join("/");
	const lines = b.file.endsWith(".rs")
		? [" use std::sync::Arc;", "+use tokio::sync::watch;", " "]
		: [' import { cn } from "@/lib/utils";', '+import { useView } from "@/app/services";', " "];
	const s = story(agent);
	s.add(you(b.ask));
	s.add(think(`Start with ${b.file} and see what's there.`), { took: 5 * SEC });
	s.add(read(b.file));
	s.add(search("TODO", folder, 3), { gap: SEC });
	s.add(read(b.file, "40-120"), { gap: SEC });
	s.add(edit(fileEdit(b.file, "modify", [{ at: 12, lines }])), { gap: 4 * SEC });
	s.add(run(b.command, "", null), { status: "running", gap: 2 * SEC });
	return s.end(now - 4 * SEC);
};

// The streaming turn -------------------------------------------------------------

const LEAD_REPLY = `W3 is done and its tests pass. Here's where the milestone stands:

- **W1 maxd core** has the replica and the socket. Reconnects are covered.
- **W2 Claude observer** maps session files to agents, including sub-agents.
- **W3 Codex observer** reads rollouts and the Codex Plus rate limits.
- **W7 Incus observer** still needs the container restart case on devbox.

The one thing blocking a merge is a flaky test in \`observe::incus\`:

\`\`\`rust
#[tokio::test]
async fn container_restart_keeps_agents() {
    let node = FakeNode::new("devbox").with_container("comet");
    node.restart("comet").await;
    assert_eq!(node.agents().await.len(), 2);
}
\`\`\`

It sleeps instead of waiting for the boot ID to change. I'll ask W7 to fix it next.`;

const LEAD_ASKS = [
	"W3 says the Codex observer is done. Check its tests and tell me what's left.",
	"How far is W7 with the Incus observer?",
	"Run the whole workspace test suite and summarize.",
];

const LEAD_OUTPUT = [
	"   Compiling maxhq-protocol v0.2.0",
	"   Compiling maxhq-daemon v0.2.0",
	"    Finished `test` profile [unoptimized + debuginfo] target(s) in 24.18s",
	"     Running unittests src/lib.rs (target/debug/deps/maxhq_daemon-8f2c)",
	"",
	"running 64 tests",
	"test observe::codex::tests::reads_rollout ... ok",
	"test observe::codex::tests::rate_limits_update_account ... ok",
	"test observe::claude::tests::maps_subagents ... ok",
	"test observe::claude::tests::tails_session_file ... ok",
	"test observe::incus::tests::lists_containers ... ok",
	"test observe::incus::tests::container_restart_keeps_agents ... ok",
	"test replica::tests::snapshot_then_deltas ... ok",
	"test replica::tests::reconnect_resends_snapshot ... ok",
	"",
	"test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.41s",
];

// Splits text into chunks of a few words, the way a model streams it.
const chunksOf = (text: string, words = 3): string[] => {
	const parts = text.split(/(?<=\s)/);
	const chunks: string[] = [];
	for (let i = 0; i < parts.length; i += words) chunks.push(parts.slice(i, i + words).join(""));
	return chunks;
};

// "M1 observe milestone": a short history, then turns that stream forever.
const leadHistory = (now: Timestamp): TimelineItem[] => {
	const s = story(STREAMING);
	s.add(you("Run M1 as seven workstreams, one sub-agent each. Keep me posted."));
	s.add(think("Seven streams, each with its own folder so they don't collide."), {
		took: 8 * SEC,
	});
	s.add(read("docs/m1-plan.md"));
	const names = ["W1 maxd core", "W2 Claude observer", "W3 Codex observer", "W4 client crate"];
	const more = ["W5 desktop backend", "W6 frontend", "W7 Incus observer"];
	[...names, ...more].forEach((name, i) => {
		s.add(
			{
				kind: "subagent",
				subagent: `agent-w${i + 1}`,
				agent_type: "general-purpose",
				description: name,
				result: i === 3 ? "The client crate is done and its tests pass." : null,
			},
			{ gap: SEC, status: i === 3 ? "completed" : "running", took: i === 3 ? 41 * MIN : 0 }
		);
	});
	s.add(say("All seven are running. W4 finished first."));
	return s.end(now - 72 * SEC);
};

// Live state ---------------------------------------------------------------

type Live = {
	readonly agent: AgentId;
	readonly items: TimelineItem[];
	readonly index: Map<ItemId, number>;
	seq: number;
	next: number;
	readonly listeners: Set<Listener>;
};

/** Settings for the sample timelines. */
export type MockTimelineOptions = {
	readonly now: () => Timestamp;
	/** Whether the lead agent streams on a timer. */
	readonly stream: boolean;
	/** How long an older page takes to arrive. */
	readonly olderDelay?: number;
};

/** The sample timelines behind the mock source. */
export type MockTimelines = {
	readonly open: (agent: AgentId, onUpdate: Listener) => () => void;
	readonly older: (agent: AgentId, before: number) => CommandResult;
	/** Marks an approval or question answered, as `maxd` does once the CLI takes the answer. */
	readonly decide: (
		attention: AttentionId,
		answer: AttentionAnswer,
		by: ClientId | null
	) => boolean;
	/** Looks for text across the agent's whole sample timeline, as `maxd` does. */
	readonly find: (agent: AgentId, query: string, at: FindAt) => TimelineFind;
	/**
	 * Adds an item to the end of an agent's timeline, as the CLI would,
	 * under `id` when given. An agent with no timeline yet starts an empty
	 * one, not a sample story.
	 */
	readonly post: (agent: AgentId, body: ItemBody, status?: ItemStatus, id?: ItemId) => void;
	/** Changes an item already in an agent's timeline, if it's there. */
	readonly amend: (
		agent: AgentId,
		id: ItemId,
		change: (item: TimelineItem) => TimelineItem
	) => void;
};

/**
 * The text a row shows that find counts, in the order the row shows it.
 * Each block counts on its own. This is near what the row's text is, which
 * is enough for the samples: Markdown counts with its marks, and a sent
 * message's pills split it.
 */
const findBlocks = (item: TimelineItem): string[] => {
	// A match never crosses a line, so each line is a block of its own.
	const lines = (text: string | null) => (text ? text.split("\n") : []);
	switch (item.kind) {
		case "user_message":
			// The app hides queued and cancelled messages, and a pill's text doesn't count.
			return item.delivery === "queued" || item.delivery === "cancelled"
				? []
				: lines(item.text.replaceAll(OBJECT_MARK, ""));
		case "assistant_message":
		case "reasoning":
		case "agent_attachments":
			return lines(item.text);
		case "plan":
			return lines(item.markdown);
		case "command":
			return [...lines(item.command), ...lines(item.output)];
		case "tool_call":
			return [...lines(item.input), ...lines(item.output)];
		case "subagent":
			return [...lines(item.description), ...lines(item.result)];
		case "error":
		case "notice":
			return lines(item.message);
		default:
			return [];
	}
};

/** The agent asked to show the release plan. The file opens live, after its first open. */
const showPlan = (now: Timestamp): TimelineItem[] => {
	const s = story(OPENS_LIVE);
	s.add(you("Show me the release plan."));
	s.add(think("The plan is in docs, so open it for them."), { took: 2 * SEC });
	return s.end(now - 3 * SEC);
};

const build = (agent: AgentId, now: Timestamp): TimelineItem[] => {
	switch (agent) {
		case OPENS_LIVE:
			return showPlan(now);
		case "agent-usage-popover":
			return usagePopover(now);
		case "agent-hdr":
			return hdrToneMapping(now);
		case "agent-release":
			return releaseChecklist(now);
		case "agent-sync-drift":
			return syncDrift(now);
		case LONG_HISTORY:
			return longHistory(now, 10_000);
		case STREAMING:
			return leadHistory(now);
		default:
			return WORKING.has(agent) ? worker(agent, now) : brief(agent, now);
	}
};

// Fills in the time an approval was answered, which a story leaves at zero.
const settle = (items: TimelineItem[]): TimelineItem[] =>
	items.map((item) =>
		item.kind === "approval" && item.decided && item.decided.at === 0
			? {
					...item,
					decided: { ...item.decided, at: item.completed_at ?? item.started_at ?? 0 },
				}
			: item
	);

const answersFor = (item: Extract<TimelineItem, { kind: "question" }>, answer: AttentionAnswer) => {
	if (answer.type === "questions") return answer.answers;
	const first = item.questions[0];
	if (!first || answer.type === "dismiss") return [];
	return [{ id: first.id, values: [answer.type === "text" ? answer.text : answer.id] }];
};

/** Sample timelines for every agent in the sample view. */
export const createMockTimelines = (options: MockTimelineOptions): MockTimelines => {
	const { now, stream } = options;
	const olderDelay = options.olderDelay ?? 120;
	const lives = new Map<AgentId, Live>();
	let timer: ReturnType<typeof setInterval> | null = null;
	let steps: (() => TimelineEvent[])[] = [];
	let round = 0;
	let shown = false;

	const liveOf = (agent: AgentId): Live => {
		let live = lives.get(agent);
		if (!live) {
			const items = settle(build(agent, now()));
			live = {
				agent,
				items,
				index: new Map(items.map((item, i) => [item.id, i])),
				seq: 1,
				next: items.length,
				listeners: new Set(),
			};
			lives.set(agent, live);
		}
		return live;
	};

	const send = (live: Live, update: TimelineUpdate) => {
		// Only changes are live. A reset or a page holds what came before.
		for (const listener of [...live.listeners]) listener(update, update.type === "events");
	};

	const emit = (live: Live, events: TimelineEvent[]) => {
		if (events.length === 0) return;
		for (const event of events) {
			if (event.type === "upsert") {
				const at = live.index.get(event.item.id);
				if (at === undefined) {
					live.index.set(event.item.id, live.items.length);
					live.items.push(event.item);
				} else live.items[at] = event.item;
			} else if (event.type === "append") {
				const at = live.index.get(event.id);
				const item = at === undefined ? undefined : live.items[at];
				if (item && at !== undefined)
					live.items[at] = withText(item, event.field, event.text);
			} else {
				const at = live.index.get(event.id);
				if (at === undefined) continue;
				live.items.splice(at, 1);
				live.index.clear();
				live.items.forEach((item, i) => live.index.set(item.id, i));
			}
		}
		live.seq += 1;
		send(live, { type: "events", seq: live.seq, events });
	};

	const current = (live: Live, id: ItemId): TimelineItem | undefined => {
		const at = live.index.get(id);
		return at === undefined ? undefined : live.items[at];
	};

	// The steps of one streamed turn for the lead agent, a tick each.
	const leadTurn = (live: Live): (() => TimelineEvent[])[] => {
		const out: (() => TimelineEvent[])[] = [];
		const turn = `${live.agent}:live-${round}`;
		let n = 0;
		const mint = (body: ItemBody, status: ItemStatus = "running"): TimelineItem => ({
			id: `${turn}:${n++}`,
			agent: live.agent,
			turn,
			parent: null,
			ordinal: live.next++,
			status,
			started_at: now(),
			completed_at: OPEN.has(status) ? null : now(),
			...body,
		});
		const done = (id: ItemId, change: (item: TimelineItem) => TimelineItem = (i) => i) => {
			const item = current(live, id);
			if (!item) return [];
			return [
				{
					type: "upsert" as const,
					item: { ...change(item), status: "completed" as const, completed_at: now() },
				},
			];
		};
		const wait = (ticks: number) => {
			for (let i = 0; i < ticks; i++) out.push(() => []);
		};
		const streamed = (kind: "reasoning" | "assistant_message", text: string) => {
			let id = "";
			out.push(() => {
				const item = mint({ kind, text: "", streaming: true });
				id = item.id;
				return [{ type: "upsert", item }];
			});
			for (const chunk of chunksOf(text)) {
				out.push(() => [{ type: "append", id, field: "text", text: chunk }]);
			}
			out.push(() =>
				done(id, (item) =>
					item.kind === "reasoning" || item.kind === "assistant_message"
						? { ...item, streaming: false }
						: item
				)
			);
		};
		const quick = (body: ItemBody, ticks: number) => {
			let id = "";
			out.push(() => {
				const item = mint(body);
				id = item.id;
				return [{ type: "upsert", item }];
			});
			wait(ticks);
			out.push(() => done(id));
		};

		const ask = pick(LEAD_ASKS, round);
		out.push(() => [{ type: "upsert", item: mint(you(ask), "completed") }]);
		wait(10);
		streamed(
			"reasoning",
			"W3 reported done, so the Codex observer tests come first. Then the Incus observer, since W7 is the last one still busy, and a full workspace run to make sure nothing else broke."
		);
		quick(read("crates/daemon/src/observe/codex.rs", "1-180"), 4);
		quick(search("container_restart", "crates/daemon", 3), 3);
		quick(read("crates/daemon/src/observe/incus.rs"), 4);

		let sub = "";
		out.push(() => {
			const item = mint({
				kind: "subagent",
				subagent: "agent-w7",
				agent_type: "general-purpose",
				description: "W7 Incus observer: fix the restart test",
				result: null,
			});
			sub = item.id;
			return [{ type: "upsert", item }];
		});

		let cmd = "";
		out.push(() => {
			const item = mint(run("cargo test --workspace", "", null));
			cmd = item.id;
			return [{ type: "upsert", item }];
		});
		for (const line of LEAD_OUTPUT) {
			out.push(() => [{ type: "append", id: cmd, field: "output", text: `${line}\n` }]);
			wait(2);
		}
		out.push(() =>
			done(cmd, (item) => (item.kind === "command" ? { ...item, exit_code: 0 } : item))
		);

		streamed("assistant_message", LEAD_REPLY);
		out.push(() =>
			done(sub, (item) =>
				item.kind === "subagent"
					? { ...item, result: "The restart test waits for the boot ID now." }
					: item
			)
		);
		out.push(() => [{ type: "upsert", item: mint(summary(41 * SEC, 121_000), "completed") }]);
		wait(60);
		return out;
	};

	const tick = () => {
		const live = lives.get(STREAMING);
		if (!live) return;
		if (steps.length === 0) {
			steps = leadTurn(live);
			round += 1;
		}
		const step = steps.shift();
		if (step) emit(live, step());
	};

	const startStreaming = () => {
		if (!stream || timer) return;
		timer = setInterval(tick, STREAM_MS);
	};

	const stopStreaming = () => {
		if (timer) clearInterval(timer);
		timer = null;
	};

	const post = (
		agent: AgentId,
		body: ItemBody,
		status: ItemStatus = "completed",
		id?: ItemId
	) => {
		let live = lives.get(agent);
		if (!live) {
			live = {
				agent,
				items: [],
				index: new Map(),
				seq: 1,
				next: 0,
				listeners: new Set(),
			};
			lives.set(agent, live);
		}
		const at = now();
		const ordinal = live.next++;
		const item: TimelineItem = {
			id: id ?? `${agent}:posted-${ordinal}`,
			agent,
			turn: null,
			parent: null,
			ordinal,
			status,
			started_at: at,
			completed_at: OPEN.has(status) ? null : at,
			...body,
		};
		emit(live, [{ type: "upsert", item }]);
	};

	const amend = (agent: AgentId, id: ItemId, change: (item: TimelineItem) => TimelineItem) => {
		const live = lives.get(agent);
		const item = live && current(live, id);
		if (live && item) emit(live, [{ type: "upsert", item: change(item) }]);
	};

	return {
		open: (agent, onUpdate) => {
			const live = liveOf(agent);
			live.listeners.add(onUpdate);
			onUpdate(
				{
					type: "reset",
					seq: live.seq,
					items: live.items.slice(-PAGE),
					has_older: live.items.length > PAGE,
				},
				false
			);
			if (agent === STREAMING) startStreaming();
			if (agent === OPENS_LIVE && !shown) {
				// It shows the file once, live, as a CLI calling open_file would.
				shown = true;
				setTimeout(() => {
					post(agent, opened(OPENS_PATH, 8));
					post(agent, say("The release plan is open in the viewer at line 8."));
				}, OPENS_AFTER_MS);
			}
			return () => {
				live.listeners.delete(onUpdate);
				if (agent === STREAMING && live.listeners.size === 0) stopStreaming();
			};
		},
		older: (agent, before) => {
			const live = lives.get(agent);
			if (!live)
				return { status: "error", error: { kind: "not_found", what: `agent ${agent}` } };
			const earlier = live.items.filter((item) => item.ordinal < before);
			const page = earlier.slice(-PAGE);
			setTimeout(
				() =>
					send(live, {
						type: "older",
						items: page,
						has_older: earlier.length > page.length,
					}),
				olderDelay
			);
			return OK;
		},
		decide: (attention, answer, by) => {
			for (const live of lives.values()) {
				const item = live.items.find(
					(i) =>
						(i.kind === "approval" || i.kind === "question") &&
						i.attention === attention
				);
				if (!item) continue;
				const at = now();
				// Dismissed: turned down, with nothing answered or decided.
				const dismissed = answer.type === "dismiss";
				if (item.kind === "approval") {
					const declined =
						dismissed ||
						answer.type === "text" ||
						(answer.type === "option" && /^(deny|decline)/.test(answer.id));
					emit(live, [
						{
							type: "upsert",
							item: {
								...item,
								status: declined ? "declined" : "completed",
								completed_at: at,
								decided: dismissed ? null : { answer, by, at },
							},
						},
					]);
				} else if (item.kind === "question") {
					emit(live, [
						{
							type: "upsert",
							item: {
								...item,
								status: dismissed ? "declined" : "completed",
								completed_at: at,
								answers: dismissed ? null : answersFor(item, answer),
							},
						},
					]);
				}
				return true;
			}
			return false;
		},
		post,
		amend,
		find: (agent, query, at) => {
			const needle = findQueryOf(query);
			const counts = liveOf(agent).items.map((item) => ({
				item: item.id,
				count: findBlocks(item).reduce(
					(sum, block) => sum + countOccurrences(collapseSpace(block), needle),
					0
				),
			}));
			return selectFind(counts, at);
		},
	};
};
