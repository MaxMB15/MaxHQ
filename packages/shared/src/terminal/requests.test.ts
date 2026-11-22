// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ActionOutcome, Request, Response } from "../bindings";
import { mockSource } from "../data/mock-source";
import { NO_TERMINALS, noTerminals } from "../data/mock-terminals";
import { asker } from "../data/request-source";
import type { ActionResult } from "../data/source";
import { NOW } from "../test/build";
import { all, applySnapshot } from "../view/replica";

import { openedTerminal, openShell, runCommand, terminalCalls } from "./requests";
import { terminalById, terminalsOf } from "./select";

const SIZE = { cols: 120, rows: 32 };

const DONE: ActionOutcome = { status: "done" };

/** Calls that keep each request and answer that it's done. */
const recorder = () => {
	const sent: Request[] = [];
	const ask = asker(async (request) => {
		sent.push(request);
		return { status: "ok", data: { kind: request.kind, value: DONE } as Response };
	});
	return { sent, calls: terminalCalls(ask) };
};

describe("terminal calls", () => {
	it("opens a shell, or runs a command typed after ! or !!", async () => {
		const { sent, calls } = recorder();
		await openShell(calls, "agent-1", SIZE);
		await runCommand(calls, "agent-1", "pnpm test", { private: false, size: SIZE });
		await runCommand(calls, "agent-1", "cat .env", { private: true, size: SIZE });
		expect(sent).toEqual([
			{ kind: "terminal_open", agent: "agent-1", runs: { type: "shell" }, ...SIZE },
			{
				kind: "terminal_open",
				agent: "agent-1",
				runs: { type: "run", command: "pnpm test", private: false },
				...SIZE,
			},
			{
				kind: "terminal_open",
				agent: "agent-1",
				runs: { type: "run", command: "cat .env", private: true },
				...SIZE,
			},
		]);
	});

	it("reads the new terminal from an open's answer", () => {
		const opened: ActionResult = {
			status: "ok",
			data: { status: "terminal_opened", terminal: "terminal-1" },
		};
		expect(openedTerminal(opened)).toBe("terminal-1");
		expect(openedTerminal({ status: "ok", data: { status: "done" } })).toBeNull();
		expect(
			openedTerminal({ status: "error", error: { kind: "not_connected" } } as ActionResult)
		).toBeNull();
	});

	it("says a maxd without terminals can't open one", async () => {
		expect(await openShell(noTerminals, "agent-1", SIZE)).toEqual({
			status: "ok",
			data: { status: "refused", reason: NO_TERMINALS },
		});
	});
});

describe("sample terminals", () => {
	const setup = async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		const view = async () => applySnapshot(await source.snapshot());
		const agent = all(await view(), "agent")[0];
		if (!agent) throw new Error("the samples have no agent");
		return { source, view, agent: agent.id };
	};

	it("opens a shell as a tab, renames it and closes it", async () => {
		const { source, view, agent } = await setup();
		const terminal = openedTerminal(await openShell(source, agent, SIZE));
		if (!terminal) throw new Error("no terminal opened");
		expect(terminalsOf(await view(), agent).map((t) => t.id)).toEqual([terminal]);
		expect(terminalById(await view(), terminal)).toMatchObject({
			title: "zsh",
			status: { state: "running" },
			...SIZE,
		});

		await source.renameTerminal(terminal, "dev server");
		await source.resizeTerminal(terminal, { cols: 100, rows: 30 });
		expect(terminalById(await view(), terminal)).toMatchObject({
			title: "dev server",
			cols: 100,
			rows: 30,
		});
		await source.renameTerminal(terminal, "");
		expect(terminalById(await view(), terminal)?.title).toBe("zsh");
		expect(await source.moveTerminal(terminal)).toMatchObject({
			data: { status: "refused" },
		});

		await source.closeTerminal(terminal);
		expect(terminalById(await view(), terminal)).toBeUndefined();
		expect(await source.closeTerminal(terminal)).toMatchObject({
			status: "error",
			error: { kind: "not_found" },
		});
	});

	it("keeps a command out of the drawer until it moves, and ends it on Stop", async () => {
		const { source, view, agent } = await setup();
		const result = await runCommand(source, agent, "pnpm dev", { private: false, size: SIZE });
		const terminal = openedTerminal(result);
		if (!terminal) throw new Error("no terminal opened");
		expect(terminalById(await view(), terminal)?.title).toBe("pnpm dev");
		expect(terminalsOf(await view(), agent)).toEqual([]);

		await source.interruptTerminal(terminal);
		expect(terminalById(await view(), terminal)?.status).toEqual({
			state: "exited",
			code: null,
			signal: 2,
			stopped: true,
		});

		expect(await source.moveTerminal(terminal)).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		expect(terminalsOf(await view(), agent).map((t) => t.id)).toEqual([terminal]);
	});

	it("can't open a terminal for an agent it doesn't have", async () => {
		const { source } = await setup();
		expect(await openShell(source, "agent-gone", SIZE)).toMatchObject({
			status: "error",
			error: { kind: "not_found" },
		});
	});
});
