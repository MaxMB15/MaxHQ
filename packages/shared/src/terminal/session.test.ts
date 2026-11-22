// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	type ActionOutcome,
	type DeviceLink,
	type Entity,
	type Request,
	type Response,
	SESSIONS_SINCE,
	type SessionBlock,
	type Terminal,
	TERMINALS_SINCE,
	type TimelineItem,
} from "../bindings";
import { asker } from "../data/request-source";
import { entities, link, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	budgetLines,
	cardRows,
	cardState,
	cardSummary,
	fillSession,
	freshNote,
	openCardOf,
	sessionCalls,
	sessionCardOf,
	sessionFolder,
	sessionPill,
	sessionPillLabel,
	sessionPillLook,
	sessionRanOf,
	sessionTerminalOf,
	supportsSessions,
	TYPED_FOLD,
	type UserSessionItem,
	wantsPill,
} from "./session";

const SIZE = { cols: 120, rows: 32 };

const DONE: ActionOutcome = { status: "done" };

const recorder = () => {
	const sent: Request[] = [];
	const ask = asker(async (request) => {
		sent.push(request);
		return { status: "ok", data: { kind: request.kind, value: DONE } as Response };
	});
	return { sent, calls: sessionCalls(ask) };
};

const block = (id: number, over: Partial<SessionBlock> = {}): SessionBlock => ({
	id,
	command: `step ${id}`,
	cwd: "~/code/example-app",
	cwd_after: null,
	private: false,
	process: { state: "exited", code: 0, signal: null },
	started_at: NOW - 10 * MIN + id * MIN,
	duration_ms: 1000,
	lines: 1,
	output: `line of ${id}`,
	output_truncated: false,
	secret_prompts: 0,
	start: null,
	...over,
});

const card = (
	id: string,
	blocks: SessionBlock[],
	over: Partial<Pick<UserSessionItem, "sealed" | "sent" | "terminal">> = {}
): UserSessionItem => ({
	id,
	agent: "agent-1",
	turn: null,
	parent: null,
	ordinal: 0,
	status: "completed",
	started_at: NOW - 10 * MIN,
	completed_at: null,
	kind: "user_session",
	terminal: "session-1",
	device: "dev-mac",
	blocks,
	sealed: false,
	sent: false,
	...over,
});

const timeline = (...items: TimelineItem[]) => ({
	items: new Map(items.map((item) => [item.id, item])),
	order: items.map((item) => item.id),
});

/** The sample's four commands: a cd, an install, a private sudo and a failing test. */
const SAMPLE = [
	block(1, {
		command: "cd apps/web",
		cwd_after: "~/code/example-app/apps/web",
		output: "",
		lines: 0,
	}),
	block(2, { command: "pnpm install", output: "Lockfile is up to date\nDone in 4.2s", lines: 2 }),
	block(3, {
		command: "sudo systemctl restart nginx",
		private: true,
		output: "[sudo] password for me: 🔒 sent",
		secret_prompts: 1,
	}),
	block(4, {
		command: "pnpm test",
		process: { state: "exited", code: 1, signal: null },
		output: "\u001b[31mFAIL\u001b[0m src/app.test.ts\n1 failed",
		lines: 2,
	}),
];

describe("session calls", () => {
	it("sends each request as the protocol has it", async () => {
		const { sent, calls } = recorder();
		await calls.sessionRun("agent-1", "pnpm test", {
			private: false,
			fresh: false,
			size: SIZE,
		});
		await calls.sessionRun("agent-1", "cat .env", { private: true, fresh: true, size: SIZE });
		await calls.sessionShare("agent-1", "card-1", 3, true);
		await calls.sessionCd("agent-1", "~/code/example-app/apps");
		await calls.sessionEnd("agent-1");
		await calls.sessionEnd("agent-1", true);
		expect(sent).toEqual([
			{
				kind: "session_run",
				agent: "agent-1",
				command: "pnpm test",
				private: false,
				fresh: false,
				...SIZE,
			},
			{
				kind: "session_run",
				agent: "agent-1",
				command: "cat .env",
				private: true,
				fresh: true,
				...SIZE,
			},
			{ kind: "session_share", agent: "agent-1", item: "card-1", block: 3, private: true },
			{ kind: "session_cd", agent: "agent-1", path: "~/code/example-app/apps" },
			{ kind: "session_end", agent: "agent-1", force: false },
			{ kind: "session_end", agent: "agent-1", force: true },
		]);
	});

	it("reads the block a run made, and nothing from a fresh shell", () => {
		expect(
			sessionRanOf({
				status: "ok",
				data: { status: "session_ran", terminal: "session-1", item: "card-1", block: 4 },
			})
		).toEqual({ terminal: "session-1", item: "card-1", block: 4 });
		expect(
			sessionRanOf({ status: "ok", data: { status: "terminal_opened", terminal: "t-2" } })
		).toBeNull();
		expect(
			sessionRanOf({ status: "ok", data: { status: "refused", reason: "No." } })
		).toBeNull();
	});
});

const CAN = { transfers: true, tunnels: true, workspaces: false, terminals: true };

const macAt = (protocol: number, terminals = true): DeviceLink =>
	link({
		id: "mac",
		name: "Mac",
		device: "dev-mac",
		route: { type: "local" },
		capabilities: { ...CAN, terminals },
		protocol,
	});

const terminal = (over: Partial<Terminal> & { id: string }): Entity => ({
	entity: "terminal",
	agent: "agent-1",
	device: "dev-mac",
	kind: { type: "session" },
	moved: false,
	title: "zsh",
	cwd: "~/code/example-app",
	status: { state: "running" },
	password_prompt: false,
	started_at: NOW - 10 * MIN,
	cols: 120,
	rows: 32,
	...over,
});

const viewOf = (list: Entity[], links: DeviceLink[]) =>
	applySnapshot({ seq: 1, entities: [...entities({ links }), ...list] });

describe("the session in the view", () => {
	it("needs terminals and protocol 48", () => {
		expect(supportsSessions(viewOf([], [macAt(SESSIONS_SINCE)]), "dev-mac")).toBe(true);
		expect(supportsSessions(viewOf([], [macAt(TERMINALS_SINCE)]), "dev-mac")).toBe(false);
		expect(supportsSessions(viewOf([], [macAt(SESSIONS_SINCE, false)]), "dev-mac")).toBe(false);
		expect(supportsSessions(viewOf([], []), "dev-mac")).toBe(false);
	});

	it("finds the agent's running session terminal, not its shells", () => {
		const view = viewOf(
			[
				terminal({ id: "shell-1", kind: { type: "shell" } }),
				terminal({ id: "ended", status: { state: "exited", code: 0, signal: null } }),
				terminal({ id: "session-1" }),
				terminal({ id: "other", agent: "agent-2" }),
			],
			[macAt(SESSIONS_SINCE)]
		);
		expect(sessionTerminalOf(view, "agent-1")?.id).toBe("session-1");
		expect(sessionTerminalOf(view, "agent-3")).toBeUndefined();
	});
});

describe("cards", () => {
	it("gives the last card a message hasn't sealed", () => {
		const sealed = card("card-1", SAMPLE, { sealed: true, sent: true });
		const open = card("card-2", [block(5)]);
		expect(openCardOf(timeline(sealed, open))?.id).toBe("card-2");
		expect(openCardOf(timeline(sealed))).toBeUndefined();
		expect(sessionCardOf(timeline(sealed, open), "card-1")?.blocks).toHaveLength(4);
		expect(sessionCardOf(timeline(sealed, open), "nope")).toBeUndefined();
	});

	it("follows the folder the last command left the session in", () => {
		const session = { id: "session-1", cwd: "~/code/example-app" };
		expect(sessionFolder(session, timeline())).toBe("~/code/example-app");
		expect(sessionFolder(session, timeline(card("card-1", SAMPLE.slice(0, 1))))).toBe(
			"~/code/example-app/apps/web"
		);
		expect(
			sessionFolder(
				session,
				timeline(card("card-1", SAMPLE.slice(0, 1)), card("card-2", [block(5)]))
			)
		).toBe("~/code/example-app");
		expect(
			sessionFolder(
				session,
				timeline(card("card-1", SAMPLE.slice(0, 1), { terminal: "an-older-session" }))
			)
		).toBe("~/code/example-app");
		expect(sessionFolder(undefined, timeline())).toBeNull();
	});

	it("reads open, sent and not sent", () => {
		expect(cardState({ sealed: false, sent: false })).toBe("open");
		expect(cardState({ sealed: true, sent: true })).toBe("sent");
		expect(cardState({ sealed: true, sent: false })).toBe("not_sent");
	});

	it("counts the blocks the agent sees, and only their failures", () => {
		const failedPrivately = block(5, {
			private: true,
			process: { state: "exited", code: 2, signal: null },
		});
		expect(cardSummary(card("card-1", [...SAMPLE, failedPrivately]))).toEqual({
			total: 5,
			shared: 3,
			failed: 1,
			running: false,
			state: "open",
		});
	});
});

describe("the pill", () => {
	it("is red with a failed shared block and says how many", () => {
		const look = sessionPillLook(card("card-1", SAMPLE));
		expect(look).toEqual({ total: 4, shared: 3, failed: 1, running: false, tone: "failed" });
		expect(sessionPillLabel(look)).toBe("Agent shell · 1 failed");
	});

	it("spins while a shared block runs, but not for a private one", () => {
		const running = block(5, { process: { state: "running" }, duration_ms: null });
		expect(sessionPillLook(card("card-1", [block(1), running])).running).toBe(true);
		expect(
			sessionPillLook(card("card-1", [block(1), { ...running, private: true }])).running
		).toBe(false);
		const look = sessionPillLook(card("card-1", [block(1), block(2)]));
		expect(look.tone).toBe("normal");
		expect(sessionPillLabel(look)).toBe("Agent shell");
	});

	it("belongs in the composer only while the card is open and shares a block", () => {
		expect(wantsPill(card("card-1", SAMPLE))).toBe(true);
		expect(wantsPill(card("card-1", [SAMPLE[2]!]))).toBe(false);
		expect(wantsPill(card("card-1", SAMPLE, { sealed: true }))).toBe(false);
	});

	it("starts empty and names its card", () => {
		expect(sessionPill(card("card-1", SAMPLE), "Home Mac")).toEqual({
			type: "session",
			device_name: "Home Mac",
			blocks: [],
			item: "card-1",
		});
	});
});

describe("the card's rows", () => {
	const typed = (id: number) => block(id, { typed: true, private: true });
	const shape = (blocks: SessionBlock[]) =>
		cardRows(blocks).map((row) =>
			row.kind === "block"
				? `${row.index}:${row.block.id}`
				: `${row.index}:[${row.blocks.map((each) => each.id).join(",")}]`
		);

	it("shows each block on its own row, typed or not, up to the fold", () => {
		expect(TYPED_FOLD).toBe(3);
		expect(shape([block(1), typed(2), typed(3), typed(4), block(5)])).toEqual([
			"0:1",
			"1:2",
			"2:3",
			"3:4",
			"4:5",
		]);
		expect(shape([])).toEqual([]);
	});

	it("folds a run of more than three typed blocks into one row", () => {
		expect(
			shape([block(1), typed(2), typed(3), typed(4), typed(5), block(6), typed(7)])
		).toEqual(["0:1", "1:[2,3,4,5]", "5:6", "6:7"]);
		expect(shape([typed(1), typed(2), typed(3), typed(4)])).toEqual(["0:[1,2,3,4]"]);
	});

	it("never folds blocks you ran with `!`, however many", () => {
		const ran = [1, 2, 3, 4, 5, 6].map((id) => block(id));
		expect(cardRows(ran).every((row) => row.kind === "block")).toBe(true);
	});

	it("takes the fold as a choice", () => {
		expect(cardRows([typed(1), typed(2)], 1).map((row) => row.kind)).toEqual(["typed"]);
	});
});

describe("the budget", () => {
	it("takes each block's last 200 lines", () => {
		expect(budgetLines([10, 500, 0])).toEqual([10, 200, 0]);
	});

	it("keeps 600 lines in all, the oldest giving up lines first", () => {
		expect(budgetLines([200, 200, 200, 200])).toEqual([0, 200, 200, 200]);
		expect(budgetLines([150, 200, 200, 200])).toEqual([0, 200, 200, 200]);
		expect(budgetLines([200, 200, 200, 100])).toEqual([100, 200, 200, 100]);
		expect(budgetLines([300, 300, 300, 300, 300])).toEqual([0, 0, 200, 200, 200]);
	});
});

describe("filling the pill at send", () => {
	it("leaves private blocks out and strips the styling", () => {
		const filled = fillSession(
			sessionPill(card("card-1", SAMPLE), "Home Mac"),
			card("card-1", SAMPLE),
			{
				now: NOW,
			}
		);
		expect(filled.item).toBe("card-1");
		expect(filled.blocks.map((b) => b.command)).toEqual([
			"cd apps/web",
			"pnpm install",
			"pnpm test",
		]);
		expect(JSON.stringify(filled)).not.toContain("sudo");
		expect(JSON.stringify(filled)).not.toContain("🔒");
		expect(filled.blocks[0]).toMatchObject({
			cwd_after: "~/code/example-app/apps/web",
			exit_code: 0,
			running: false,
			text: "",
		});
		expect(filled.blocks[2]).toEqual({
			command: "pnpm test",
			cwd: "~/code/example-app",
			cwd_after: null,
			exit_code: 1,
			running: false,
			duration_ms: 1000,
			lines_total: 2,
			text: "FAIL src/app.test.ts\n1 failed",
		});
	});

	it("keeps the last lines within the budget and the whole count", () => {
		const long = (from: number) =>
			Array.from({ length: 300 }, (_, i) => `line ${from + i}`).join("\n");
		const blocks = [1, 2, 3, 4].map((id) => block(id, { output: long(id * 1000), lines: 300 }));
		const filled = fillSession(
			sessionPill({ id: "card-1" }, "Home Mac"),
			card("card-1", blocks),
			{
				now: NOW,
			}
		);
		expect(filled.blocks.map((b) => (b.text === "" ? 0 : b.text.split("\n").length))).toEqual([
			0, 200, 200, 200,
		]);
		expect(filled.blocks.map((b) => b.lines_total)).toEqual([300, 300, 300, 300]);
		expect(filled.blocks[1]!.text.split("\n")[0]).toBe("line 2100");
		expect(filled.blocks[3]!.text.split("\n").at(-1)).toBe("line 4299");
	});

	it("sends a running block with what it wrote so far, from the live view when there is one", () => {
		const running = block(2, {
			process: { state: "running" },
			duration_ms: null,
			started_at: NOW - 3000,
			output: "",
			lines: 0,
		});
		const item = card("card-1", [block(1), running]);
		const filled = fillSession(sessionPill(item, "Home Mac"), item, {
			now: NOW,
			live: (b) => (b.id === 2 ? { text: "building\nstill building", total: 2 } : null),
		});
		expect(filled.blocks[1]).toMatchObject({
			running: true,
			exit_code: null,
			duration_ms: 3000,
			lines_total: 2,
			text: "building\nstill building",
		});
		const without = fillSession(sessionPill(item, "Home Mac"), item, { now: NOW });
		expect(without.blocks[1]).toMatchObject({ running: true, text: "" });
	});
});

describe("why a command ran in a fresh shell", () => {
	it("names the setting and the device when integration is off", () => {
		expect(freshNote("integration_off")).toEqual({
			key: "Sessions need {{setting}}, which is off on {{device}}.",
			setting: true,
		});
	});

	it("says a line for what the session couldn't take, and nothing when you asked", () => {
		expect(freshNote("no_marks")).toEqual({
			key: "Your shell didn't report its commands, so this ran on its own.",
			setting: false,
		});
		expect(freshNote("busy")?.key).toContain("busy");
		expect(freshNote("lines")?.key).toContain("more than one line");
		expect(freshNote("exec")?.key).toContain("exec");
		expect(freshNote("asked")).toBeNull();
		expect(freshNote(null)).toBeNull();
		expect(freshNote(undefined)).toBeNull();
	});
});
