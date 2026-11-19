// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { TerminalId } from "../bindings";
import { openedTerminal, openShell, runCommand } from "../terminal/requests";
import { lastByte, type TerminalEvent, stepFor } from "../terminal/stream";
import { terminalById } from "../terminal/select";
import { NOW } from "../test/build";
import { all, applySnapshot } from "../view/replica";

import { mockSource } from "./mock-source";
import { STILL_RUNNING } from "./mock-terminals";

const SIZE = { cols: 120, rows: 32 };

// Strips colors but keeps line ends and backspaces, so the checks read the
// bytes a person sees.
// eslint-disable-next-line no-control-regex
const plain = (text: string) => text.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "");

const setup = async () => {
	const source = mockSource({ now: () => NOW, tick: false });
	const stream = source.terminalStream;
	if (!stream) throw new Error("the samples have no terminal stream");
	const view = async () => applySnapshot(await source.snapshot());
	const agent = all(await view(), "agent")[0];
	if (!agent) throw new Error("the samples have no agent");
	/** Attaches and keeps what comes, the way a view would. */
	const watch = async (terminal: TerminalId, after: number | null = null) => {
		let end: number | null = null;
		let text = "";
		const events: TerminalEvent[] = [];
		stream.listen(terminal, (event) => {
			events.push(event);
			const step = stepFor(end, event);
			if (!step) return;
			if (step.clear) text = "";
			text += new TextDecoder().decode(step.bytes);
			end = step.end;
		});
		await stream.attach(terminal, after);
		return {
			events,
			text: () => plain(text),
			end: () => end,
		};
	};
	const type = (terminal: TerminalId, text: string) =>
		stream.input(terminal, new TextEncoder().encode(text));
	return { source, stream, view, agent: agent.id, watch, type };
};

describe("sample terminal stream", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("prints a prompt in the agent's folder and echoes what you type", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		const cwd = terminalById(await view(), terminal)?.cwd ?? "";
		const folder = cwd.split("/").pop();
		const out = await watch(terminal);
		expect(out.text()).toBe(`${folder} % `);

		type(terminal, "ecko\x7f\x7fho hi\r");
		expect(out.text()).toBe(`${folder} % ecko\b \b\b \bho hi\r\nhi\r\n${folder} % `);
	});

	it("runs pnpm test over a few seconds, busy and named after it, and fails", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		type(terminal, "pnpm test\r");
		expect(terminalById(await view(), terminal)).toMatchObject({
			title: "pnpm test",
			busy: true,
		});
		expect(await source.closeTerminal(terminal)).toEqual({
			status: "ok",
			data: { status: "refused", reason: STILL_RUNNING },
		});
		expect(out.text()).not.toContain("Test Files");

		await vi.advanceTimersByTimeAsync(4_000);
		expect(out.text()).toContain("Test Files  1 failed | 2 passed (3)");
		expect(terminalById(await view(), terminal)).toMatchObject({ title: "zsh", busy: false });
		expect(await source.closeTerminal(terminal)).toMatchObject({ data: { status: "done" } });
	});

	it("says a shell you named is busy while pnpm dev runs, until Ctrl-C", async () => {
		const { source, agent, type, view } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		await source.renameTerminal(terminal, "server");
		expect(terminalById(await view(), terminal)?.busy).toBe(false);

		type(terminal, "pnpm dev\r");
		await vi.advanceTimersByTimeAsync(2_000);
		expect(terminalById(await view(), terminal)).toMatchObject({ title: "server", busy: true });

		type(terminal, "\x03");
		expect(terminalById(await view(), terminal)).toMatchObject({
			title: "server",
			busy: false,
		});
		expect(await source.closeTerminal(terminal)).toMatchObject({ data: { status: "done" } });
	});

	it("asks for a password without echoing it and says it was sent", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		type(terminal, "sudo true\r");
		expect(terminalById(await view(), terminal)?.password_prompt).toBe(true);

		type(terminal, "hunter2\r");
		expect(terminalById(await view(), terminal)?.password_prompt).toBe(false);
		expect(out.text()).not.toContain("hunter2");
		expect(out.text()).toContain("Password: 🔒 sent\r\n");
	});

	it("prints a log line every second until Ctrl-C", async () => {
		const { source, agent, watch, view } = await setup();
		const result = await runCommand(source, agent, "pnpm dev", { private: false, size: SIZE });
		const terminal = openedTerminal(result);
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		const lines = () => out.text().split("[vite] hmr update").length - 1;
		await vi.advanceTimersByTimeAsync(3_000);
		expect(lines()).toBe(3);
		expect(terminalById(await view(), terminal)?.busy).toBe(true);

		await source.interruptTerminal(terminal);
		await vi.advanceTimersByTimeAsync(3_000);
		expect(lines()).toBe(3);
		expect(terminalById(await view(), terminal)).toMatchObject({
			status: { state: "exited", code: null, signal: 2 },
			busy: false,
		});
	});

	it("replays from where a view ended, and only while attached", async () => {
		const { source, stream, agent, watch, type } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		const first = await watch(terminal);
		const end = first.end();
		stream.detach(terminal);
		type(terminal, "ls\r");
		expect(first.text()).not.toContain("README.md");

		const again = await watch(terminal, lastByte(end));
		expect(again.events[0]).toMatchObject({ type: "output", offset: end });
		expect(again.text()).toContain("README.md");
		await expect(stream.attach("terminal-gone", null)).rejects.toThrow(
			"This terminal is gone."
		);
	});
});
