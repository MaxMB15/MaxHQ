// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { TerminalId } from "../bindings";
import { openedTerminal, openShell, runCommand } from "../terminal/requests";
import { lastByte, type TerminalEvent, stepFor } from "../terminal/stream";
import { deviceSettingsOf } from "../settings/device-settings";
import { terminalById } from "../terminal/select";
import { NOW } from "../test/build";
import { all, applySnapshot } from "../view/replica";

import { mockSource } from "./mock-source";
import { STILL_RUNNING } from "./mock-terminals";

const SIZE = { cols: 120, rows: 32 };

// Strips colors and shell marks but keeps line ends and backspaces, so the
// checks read the bytes a person sees.
// eslint-disable-next-line no-control-regex
const plain = (text: string) => text.replace(/\x1b\[[0-9;]*[A-Za-z]|\x1b\]133;[^\x07]*\x07/g, "");

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
			// eslint-disable-next-line no-control-regex
			marks: () => [...text.matchAll(/\x1b\]133;([^\x07]*)\x07/g)].map((found) => found[1]),
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

	it("waits for you with yes and no once ssh asks, and goes on with your answer", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(
			await runCommand(source, agent, "ssh devbox", { private: false, size: SIZE })
		);
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		expect(out.text()).toContain("(yes/no/[fingerprint])? ");
		expect(terminalById(await view(), terminal)?.waiting).toBeFalsy();

		await vi.advanceTimersByTimeAsync(1_000);
		expect(terminalById(await view(), terminal)).toMatchObject({
			waiting: true,
			choices: ["yes", "no"],
		});

		type(terminal, "ye");
		expect(terminalById(await view(), terminal)).toMatchObject({ waiting: false, choices: [] });
		type(terminal, "s\r");
		expect(out.text()).toContain("? yes\r\nWarning: Permanently added 'devbox'");
		await vi.advanceTimersByTimeAsync(1_000);
		expect(terminalById(await view(), terminal)?.status).toEqual({
			state: "exited",
			code: 0,
			signal: null,
		});
	});

	it("asks gh for a secret without echoing it, and waits for you meanwhile", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(
			await runCommand(source, agent, "gh secret set API_TOKEN", {
				private: false,
				size: SIZE,
			})
		);
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		await vi.advanceTimersByTimeAsync(1_000);
		expect(terminalById(await view(), terminal)).toMatchObject({
			password_prompt: true,
			waiting: true,
		});

		type(terminal, "s3cr3t\r");
		expect(out.text()).not.toContain("s3cr3t");
		expect(out.text()).toContain("Set Actions secret API_TOKEN for owner/repo");
		expect(terminalById(await view(), terminal)).toMatchObject({
			password_prompt: false,
			waiting: false,
			status: { state: "exited", code: 0 },
		});
	});

	it("lets python take Stop and go on, until Force stop kills it", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(
			await runCommand(source, agent, "python3", { private: false, size: SIZE })
		);
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		type(terminal, "1 + 1\r");
		await source.interruptTerminal(terminal);
		expect(out.text()).toContain("KeyboardInterrupt\r\n>>> ");
		expect(terminalById(await view(), terminal)?.status.state).toBe("running");

		expect(await source.closeTerminal(terminal, true)).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		expect(terminalById(await view(), terminal)).toBeUndefined();
	});

	it("kills what runs in a session on Force stop and says you stopped it", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		type(terminal, "python3\r");
		await source.interruptTerminal(terminal);
		expect(terminalById(await view(), terminal)?.busy).toBe(true);

		await source.interruptTerminal(terminal, true);
		expect(terminalById(await view(), terminal)).toMatchObject({
			busy: false,
			status: { state: "running" },
		});
		expect(out.text()).toMatch(/KeyboardInterrupt\r\n>>> \r\n\S+ % $/);
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
			status: { state: "exited", code: null, signal: 2, stopped: true },
			busy: false,
		});
	});

	it("takes the whole screen for vim until :wq, and says how to quit at Ctrl-C", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(
			await runCommand(source, agent, "vim notes.md", { private: false, size: SIZE })
		);
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		expect(terminalById(await view(), terminal)).toMatchObject({ full_screen: true });
		expect(out.text()).toContain("# Notes");

		type(terminal, "\x03");
		expect(out.text()).toContain("Type  :qa  and press <Enter> to exit Vim");
		expect(terminalById(await view(), terminal)?.status.state).toBe("running");

		type(terminal, ":wq\r");
		expect(terminalById(await view(), terminal)).toMatchObject({
			full_screen: false,
			status: { state: "exited", code: 0 },
		});
	});

	it("lets you pick with the arrow keys in either cursor mode, and Enter", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(
			await runCommand(source, agent, "pnpm create vite", { private: false, size: SIZE })
		);
		if (!terminal) throw new Error("no terminal opened");
		const out = await watch(terminal);
		expect(terminalById(await view(), terminal)).toMatchObject({
			reads_keys: true,
			app_cursor: true,
		});
		await vi.advanceTimersByTimeAsync(1_100);
		expect(terminalById(await view(), terminal)?.waiting).toBe(true);

		type(terminal, "\x1bOB\x1b[B\x1bOA");
		expect(terminalById(await view(), terminal)?.waiting).toBe(false);
		type(terminal, "\r");
		expect(out.text()).toContain("◇  Select a framework:\r\n│  Vue");
		expect(terminalById(await view(), terminal)).toMatchObject({
			reads_keys: false,
			app_cursor: false,
		});
		await vi.advanceTimersByTimeAsync(1_000);
		expect(terminalById(await view(), terminal)?.status).toMatchObject({
			state: "exited",
			code: 0,
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

	it("marks a shell's prompts and commands while its device's Shell integration is on", async () => {
		const { source, agent, watch, type, view } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		// Its holder lets through only the marks it adds.
		expect(terminalById(await view(), terminal)?.marks).toBe(true);
		const out = await watch(terminal);
		type(terminal, "ls\r\r");
		type(terminal, "pnpm dev\r");
		type(terminal, "\x03");
		expect(out.marks()).toEqual([
			"A",
			"B",
			"C",
			"D;0",
			"A",
			"B",
			"A",
			"B",
			"C",
			"D;130",
			"A",
			"B",
		]);

		// Off, a shell opened after the change prints none.
		const device = terminalById(await view(), terminal)?.device ?? "";
		const settings = deviceSettingsOf(await view(), device);
		await source.setDeviceSettings({ device, ...settings, shell_integration: false });
		const plainShell = openedTerminal(await openShell(source, agent, SIZE));
		if (!plainShell) throw new Error("no terminal opened");
		const quiet = await watch(plainShell);
		type(plainShell, "ls\r");
		expect(quiet.text()).toContain("README.md");
		expect(quiet.marks()).toEqual([]);
	});
});
