// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentId, InlineRef, ItemId, TerminalId, TimelineItem } from "../bindings";
import { deviceSettingsOf } from "../settings/device-settings";
import { openedTerminal } from "../terminal/requests";
import { sessionRanOf, type UserSessionItem } from "../terminal/session";
import type { UserRunItem } from "../terminal/output";
import { NOW } from "../test/build";
import { all, applySnapshot } from "../view/replica";

import { CARD_SEALED, NO_SESSIONS, noSessions, SAMPLE_SESSION } from "./mock-sessions";
import { mockSource } from "./mock-source";
import { STILL_RUNNING } from "./mock-terminals";
import { MAC } from "./mock-view";

const SIZE = { cols: 120, rows: 32 };

/** A Mac agent with no session in the samples. */
const AGENT = "agent-sidebar-sort" as AgentId;

const FOLDER = "/Users/me/Code/Github/Multi/MaxAIManager";

const setup = async (agent: AgentId = AGENT) => {
	let now = NOW;
	const source = mockSource({ now: () => now, tick: false });
	const items = new Map<string, TimelineItem>();
	await source.openTimeline(agent, (update) => {
		if (update.type === "events") {
			for (const event of update.events)
				if (event.type === "upsert") items.set(event.item.id, event.item);
		} else for (const item of update.items) items.set(item.id, item);
	});
	const card = (id: ItemId): UserSessionItem => {
		const item = items.get(id);
		if (item?.kind !== "user_session") throw new Error(`no card ${id}`);
		return item;
	};
	const runCard = (terminal: TerminalId): UserRunItem | undefined => {
		const item = items.get(`run-${terminal}`);
		return item?.kind === "user_run" ? item : undefined;
	};
	const terminals = async () => all(applySnapshot(await source.snapshot()), "terminal");
	const run = async (command: string, options: { private?: boolean; fresh?: boolean } = {}) => {
		const result = await source.sessionRun(agent, command, {
			private: options.private ?? false,
			fresh: options.fresh ?? false,
			size: SIZE,
		});
		return { result, ran: sessionRanOf(result), opened: openedTerminal(result) };
	};
	const ran = async (command: string, privately = false) => {
		const { ran: block } = await run(command, { private: privately });
		if (!block) throw new Error(`${command} didn't run in the session`);
		return block;
	};
	const later = async (ms: number) => {
		now += ms;
		await vi.advanceTimersByTimeAsync(ms);
	};
	const send = (refs: InlineRef[] = []) =>
		source.sendInput(agent, { text: "next", attachments: [], refs }, "queue");
	return { source, items, card, runCard, terminals, run, ran, later, send };
};

const sessionPillOf = (item: ItemId): InlineRef => ({
	type: "session",
	device_name: "Mac",
	blocks: [],
	item,
});

describe("sample sessions", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("opens the session with the first command and keeps its folder", async () => {
		const { card, terminals, ran } = await setup();
		const first = await ran("cd apps");
		const [session] = (await terminals()).filter(
			(terminal) => terminal.agent === AGENT && terminal.kind.type === "session"
		);
		expect(session?.id).toBe(first.terminal);
		expect(first.block).toBe(1);
		expect(card(first.item)).toMatchObject({
			status: "completed",
			terminal: first.terminal,
			sealed: false,
			blocks: [
				{
					id: 1,
					command: "cd apps",
					cwd: FOLDER,
					cwd_after: `${FOLDER}/apps`,
					process: { state: "exited", code: 0, signal: null },
				},
			],
		});

		const second = await ran("pwd");
		expect(second).toEqual({ terminal: first.terminal, item: first.item, block: 2 });
		expect(card(first.item).blocks[1]).toMatchObject({
			cwd: `${FOLDER}/apps`,
			cwd_after: null,
			output: `${FOLDER}/apps`,
		});
	});

	it("shows a block running, then how it ended and where its output starts", async () => {
		const { card, ran, later } = await setup();
		const { item } = await ran("pnpm install");
		expect(card(item)).toMatchObject({ status: "running", completed_at: null });
		expect(card(item).blocks[0]?.process).toEqual({ state: "running" });
		expect(card(item).blocks[0]?.start).toBeGreaterThan(0);

		await later(3_400);
		expect(card(item).status).toBe("completed");
		expect(card(item).blocks[0]).toMatchObject({
			process: { state: "exited", code: 0, signal: null },
			duration_ms: 3_400,
			lines: 4,
		});
		expect(card(item).blocks[0]?.output).toContain("Already up to date");
		// The marks the shell prints stay out of the block.
		expect(card(item).blocks[0]?.output).not.toContain("\x1b]133");
	});

	it("runs in a fresh shell and says why when the session can't take it", async () => {
		const { runCard, run, ran } = await setup();
		const asked = await run("pwd", { fresh: true });
		expect(asked.ran).toBeNull();
		expect(runCard(asked.opened!)?.fresh).toBe("asked");
		expect(runCard((await run("echo a\necho b")).opened!)?.fresh).toBe("lines");
		expect(runCard((await run("exec zsh")).opened!)?.fresh).toBe("exec");

		await ran("cd apps");
		await ran("pnpm install");
		const busy = await run("pwd");
		expect(runCard(busy.opened!)).toMatchObject({ fresh: "busy", cwd: `${FOLDER}/apps` });
	});

	it("runs on its own while Shell integration is off", async () => {
		const { source, runCard, run } = await setup();
		const view = applySnapshot(await source.snapshot());
		const settings = deviceSettingsOf(view, MAC);
		await source.setDeviceSettings({ device: MAC, ...settings, shell_integration: false });
		const { opened } = await run("pwd");
		expect(runCard(opened!)?.fresh).toBe("integration_off");
	});

	it("keeps a private block's password out and counts it", async () => {
		const { source, card, ran } = await setup();
		const { terminal, item } = await ran("sudo ls", true);
		source.terminalStream?.input(terminal, new TextEncoder().encode("hunter2\r"));
		const block = card(item).blocks[0];
		expect(block).toMatchObject({ private: true, secret_prompts: 1 });
		expect(block?.output).toBe("[sudo] password for me: 🔒 sent");
		expect(JSON.stringify(card(item))).not.toContain("hunter2");
	});

	it("shares a block until a message closes its card", async () => {
		const { source, card, ran, send } = await setup();
		const { item } = await ran("pwd");
		expect(await source.sessionShare(AGENT, item, 1, true)).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		expect(card(item).blocks[0]?.private).toBe(true);

		await send([sessionPillOf(item)]);
		expect(card(item)).toMatchObject({ sealed: true, sent: true });
		expect(await source.sessionShare(AGENT, item, 1, false)).toEqual({
			status: "ok",
			data: { status: "refused", reason: CARD_SEALED },
		});

		// The next command starts a card of its own, counting on.
		const next = await ran("pwd");
		expect(next.item).not.toBe(item);
		expect(next.block).toBe(2);
	});

	it("says a card a message didn't take along wasn't sent", async () => {
		const { card, ran, send } = await setup();
		const { item } = await ran("pwd");
		await send();
		expect(card(item)).toMatchObject({ sealed: true, sent: false });
	});

	it("opens the session in a folder, or moves it there with a shared cd", async () => {
		const { source, card, terminals, later } = await setup();
		const opened = await source.sessionCd(AGENT, `${FOLDER}/apps`);
		const terminal = openedTerminal(opened);
		const session = (await terminals()).find((each) => each.id === terminal);
		expect(session).toMatchObject({ kind: { type: "session" }, cwd: `${FOLDER}/apps` });

		const moved = sessionRanOf(await source.sessionCd(AGENT, FOLDER));
		expect(card(moved!.item).blocks[0]).toMatchObject({
			command: `cd -- '${FOLDER}'`,
			private: false,
			cwd: `${FOLDER}/apps`,
			cwd_after: FOLDER,
		});

		await source.sessionRun(AGENT, "pnpm install", {
			private: false,
			fresh: false,
			size: SIZE,
		});
		const busy = await source.sessionCd(AGENT, "~");
		expect(busy).toEqual({
			status: "ok",
			data: {
				status: "refused",
				reason: "The session is busy. Wait for pnpm install to finish, or stop it.",
			},
		});
		await later(3_400);
	});

	it("ends the session, asking first while a command runs", async () => {
		const { source, card, terminals, ran } = await setup();
		const { item, terminal } = await ran("pnpm dev");
		expect(await source.sessionEnd(AGENT)).toEqual({
			status: "ok",
			data: { status: "refused", reason: STILL_RUNNING },
		});
		expect(await source.sessionEnd(AGENT, true)).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		expect((await terminals()).some((each) => each.id === terminal)).toBe(false);
		expect(card(item).blocks[0]?.process).toEqual({
			state: "exited",
			code: null,
			signal: null,
		});
		expect(card(item).status).toBe("completed");
	});

	it("goes on with the sample's open card, and closes it with a message", async () => {
		const { card, ran, send } = await setup(SAMPLE_SESSION.agent);
		expect(card(SAMPLE_SESSION.item).blocks.map((block) => block.command)).toEqual([
			"cd apps/desktop",
			"pnpm install",
			"sudo dscacheutil -flushcache",
			"pnpm test",
			"git log --oneline -3",
			"export GH_TOKEN=•••",
		]);
		const next = await ran("pwd");
		expect(next).toEqual({
			terminal: SAMPLE_SESSION.terminal,
			item: SAMPLE_SESSION.item,
			block: SAMPLE_SESSION.next,
		});
		expect(card(SAMPLE_SESSION.item).blocks.at(-1)?.output).toBe(SAMPLE_SESSION.cwd);

		await send([sessionPillOf(SAMPLE_SESSION.item)]);
		expect(card(SAMPLE_SESSION.item)).toMatchObject({ sealed: true, sent: true });
	});

	it("adds a line you type in the session's tab as a private block, with secrets masked", async () => {
		const { source, card } = await setup(SAMPLE_SESSION.agent);
		const type = (text: string) =>
			source.terminalStream?.input(SAMPLE_SESSION.terminal, new TextEncoder().encode(text));
		const blocks = () => card(SAMPLE_SESSION.item).blocks;
		const before = blocks().length;

		type("pwd\r");
		expect(blocks().at(-1)).toMatchObject({
			id: SAMPLE_SESSION.next,
			command: "pwd",
			typed: true,
			private: true,
			output: SAMPLE_SESSION.cwd,
			process: { state: "exited", code: 0 },
		});

		type("echo --password hunter2 API_KEY='a b'\r");
		expect(blocks().at(-1)?.command).toBe("echo --password ••• API_KEY=•••");

		// A line that starts with a space stays out, as it does from a shell's history.
		type(" ls\r");
		expect(blocks()).toHaveLength(before + 2);
	});

	it("keeps what vim showed out of its block, and ends it once you quit", async () => {
		const { card, ran, source } = await setup(SAMPLE_SESSION.agent);
		const block = await ran("vim notes.md");
		const type = (text: string) =>
			source.terminalStream?.input(SAMPLE_SESSION.terminal, new TextEncoder().encode(text));
		const ofCard = () => card(block.item).blocks.find((each) => each.id === block.block);
		expect(ofCard()?.process.state).toBe("running");

		type(":q\r");
		expect(ofCard()).toMatchObject({
			process: { state: "exited", code: 0 },
			output: "",
		});
		expect(ofCard()?.typed).toBeUndefined();
	});

	it("refuses every request without sessions", async () => {
		const refused = { status: "ok", data: { status: "refused", reason: NO_SESSIONS } };
		expect(
			await noSessions.sessionRun(AGENT, "pwd", { private: false, fresh: false, size: SIZE })
		).toEqual(refused);
		expect(await noSessions.sessionShare(AGENT, "item", 1, true)).toEqual(refused);
		expect(await noSessions.sessionCd(AGENT, "~")).toEqual(refused);
		expect(await noSessions.sessionEnd(AGENT)).toEqual(refused);
	});
});
