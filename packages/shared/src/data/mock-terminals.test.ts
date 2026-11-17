// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentId, TerminalId, TimelineItem } from "../bindings";
import { openedTerminal, runCommand } from "../terminal/requests";
import type { UserRunItem } from "../terminal/output";
import { NOW } from "../test/build";
import { all, applySnapshot } from "../view/replica";

import { mockSource } from "./mock-source";

const SIZE = { cols: 120, rows: 32 };

const setup = async () => {
	let now = NOW;
	const source = mockSource({ now: () => now, tick: false });
	const agent = all(applySnapshot(await source.snapshot()), "agent")[0];
	if (!agent) throw new Error("the samples have no agent");
	const items = new Map<string, TimelineItem>();
	await source.openTimeline(agent.id, (update) => {
		if (update.type === "events") {
			for (const event of update.events)
				if (event.type === "upsert") items.set(event.item.id, event.item);
		} else for (const item of update.items) items.set(item.id, item);
	});
	/** The card of the run in `terminal`. */
	const card = (terminal: TerminalId): UserRunItem | undefined => {
		const item = [...items.values()].find(
			(i) => i.kind === "user_run" && i.terminal === terminal
		);
		return item?.kind === "user_run" ? item : undefined;
	};
	const run = async (command: string, privately = false) => {
		const result = await runCommand(source, agent.id as AgentId, command, {
			private: privately,
			size: SIZE,
		});
		const terminal = openedTerminal(result);
		if (!terminal) throw new Error("no terminal opened");
		return terminal;
	};
	const later = async (ms: number) => {
		now += ms;
		await vi.advanceTimersByTimeAsync(ms);
	};
	return { source, card, run, later };
};

describe("sample ! runs", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("shows a running card, then how it ended and what it printed", async () => {
		const { card, run, later } = await setup();
		const terminal = await run("pnpm test");
		expect(card(terminal)).toMatchObject({
			id: `run-${terminal}`,
			status: "running",
			command: "pnpm test",
			private: false,
			process: { state: "running" },
			output: "",
			completed_at: null,
		});

		await later(4_000);
		const done = card(terminal);
		expect(done).toMatchObject({
			status: "failed",
			process: { state: "exited", code: 1, signal: null },
			duration_ms: 4_000,
			output_truncated: false,
		});
		expect(done?.output).toContain("Test Files");
		expect(done?.output).toContain("\x1b[31m");
		expect(done?.output).not.toMatch(/\r|\n$/);
		expect(done?.lines).toBe(done?.output.split("\n").length);
	});

	it("completes a command that prints all at once", async () => {
		const { card, run } = await setup();
		const terminal = await run("echo hi", true);
		expect(card(terminal)).toMatchObject({
			status: "completed",
			private: true,
			process: { state: "exited", code: 0, signal: null },
			output: "hi",
			lines: 1,
		});
	});

	it("never keeps the password you send at a prompt", async () => {
		const { source, card, run } = await setup();
		const terminal = await run("sudo true");
		source.terminalStream?.input(terminal, new TextEncoder().encode("hunter2\r"));
		const done = card(terminal);
		expect(done?.output).toBe("Password: 🔒 sent");
		expect(JSON.stringify(done)).not.toContain("hunter2");
	});

	it("marks a moved run, and ends one closed while it runs", async () => {
		const { source, card, run } = await setup();
		const terminal = await run("pnpm dev");
		await source.moveTerminal(terminal);
		expect(card(terminal)?.moved).toBe(true);

		await source.closeTerminal(terminal, true);
		expect(card(terminal)).toMatchObject({
			status: "failed",
			process: { state: "exited", code: null, signal: 1 },
		});
		expect(card(terminal)?.output).toContain("VITE");
	});

	it("ends with Ctrl-C as interrupted", async () => {
		const { source, card, run, later } = await setup();
		const terminal = await run("pnpm dev");
		await later(2_000);
		await source.interruptTerminal(terminal);
		expect(card(terminal)).toMatchObject({
			status: "failed",
			process: { state: "exited", code: null, signal: 2 },
			duration_ms: 2_000,
		});
		expect(card(terminal)?.output).toMatch(/hmr update[^]*\^C$/);
	});
});
