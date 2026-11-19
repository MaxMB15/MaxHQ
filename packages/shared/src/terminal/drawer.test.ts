// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Terminal } from "../bindings";
import { agent, entities, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { pickedOutput, runningCommand, runningCommands, runsCommand, tabTitle } from "./drawer";
import type { TerminalEntity } from "./select";

const terminal = (over: Partial<Terminal> = {}): TerminalEntity => ({
	entity: "terminal",
	id: "t-1",
	agent: "agent-1",
	device: "dev-mac",
	kind: { type: "shell" },
	moved: false,
	title: "zsh",
	cwd: "~/code/example-app",
	status: { state: "running" },
	busy: false,
	password_prompt: false,
	started_at: NOW - 10 * MIN,
	cols: 120,
	rows: 32,
	...over,
});

const run = (over: Partial<Terminal> = {}) =>
	terminal({ kind: { type: "run", command: "pnpm test", private: false }, ...over });

const exited = { state: "exited", code: 0, signal: null } as const;

describe("runsCommand", () => {
	it("says a shell runs a command while it's busy, whatever its title", () => {
		for (const title of ["zsh", "pnpm dev", "~/code/example-app", "server", ""]) {
			expect(runsCommand(terminal({ title }))).toBe(false);
			expect(runsCommand(terminal({ title, busy: true }))).toBe(true);
		}
		// A device that doesn't say reads as waiting at the prompt.
		expect(runsCommand({ kind: { type: "shell" }, status: { state: "running" } })).toBe(false);
	});

	it("says a command you ran runs until it exits, busy or not", () => {
		expect(runsCommand(run())).toBe(true);
		expect(runsCommand(run({ busy: true }))).toBe(true);
		expect(runsCommand(run({ busy: true, status: exited }))).toBe(false);
		expect(runsCommand(terminal({ busy: true, status: exited }))).toBe(false);
	});
});

describe("runningCommand", () => {
	it("names what a busy shell runs by its tab, and what you ran by its command", () => {
		expect(runningCommand(terminal({ title: "pnpm dev", busy: true }))).toBe("pnpm dev");
		expect(runningCommand(terminal({ title: "server", busy: true }))).toBe("server");
		expect(runningCommand(terminal({ title: "", busy: true }))).toBe("example-app");
		expect(runningCommand(run({ title: "vitest" }))).toBe("pnpm test");
	});

	it("names nothing while a shell waits, or once it exits", () => {
		expect(runningCommand(terminal({ title: "pnpm dev" }))).toBeNull();
		expect(
			runningCommand(terminal({ title: "pnpm dev", busy: true, status: exited }))
		).toBeNull();
		expect(runningCommand(run({ status: exited }))).toBeNull();
	});
});

describe("runningCommands", () => {
	const view = applySnapshot({
		seq: 1,
		entities: [
			...entities({
				agents: [
					agent({ id: "lead" }),
					agent({ id: "sub", parent: "lead" }),
					agent({ id: "other" }),
				],
			}),
			terminal({ id: "t-dev", agent: "lead", title: "pnpm dev", busy: true }),
			terminal({ id: "t-idle", agent: "lead", title: "cargo watch" }),
			run({ id: "t-sub", agent: "sub" }),
			terminal({ id: "t-again", agent: "sub", title: "pnpm dev", busy: true }),
			terminal({ id: "t-named", agent: "lead", title: "server", busy: true }),
			terminal({ id: "t-other", agent: "other", title: "cargo watch", busy: true }),
		],
	});

	it("names each command still running in the agents' and their sub-agents' terminals once", () => {
		expect(runningCommands(view, new Set(["lead"]))).toEqual([
			"pnpm dev",
			"pnpm test",
			"server",
		]);
		expect(runningCommands(view, new Set(["gone"]))).toEqual([]);
	});
});

describe("pickedOutput", () => {
	it("makes an Output pill with the picked lines and the terminal it came from", () => {
		const picked = { text: "served stale=true\nshowing banner", lines: { first: 6, last: 7 } };
		expect(pickedOutput(terminal({ title: "pnpm dev", busy: true }), "Mac", picked, 9)).toEqual(
			{
				type: "output",
				command: "pnpm dev",
				device_name: "Mac",
				cwd: "~/code/example-app",
				exit_code: null,
				running: true,
				ran_at: NOW - 10 * MIN,
				duration_ms: null,
				lines_total: 9,
				text: picked.text,
				terminal: "t-1",
				lines: { first: 6, last: 7 },
			}
		);
	});

	it("says how a finished terminal exited, and names a shell by its tab", () => {
		const done = terminal({ status: { state: "exited", code: 1, signal: null } });
		const ref = pickedOutput(done, "devbox", { text: "x", lines: { first: 2, last: 2 } }, 1);
		expect(ref).toMatchObject({ command: "zsh", exit_code: 1, running: false, lines_total: 2 });
		expect(tabTitle(terminal({ title: "" }))).toBe("example-app");
	});

	it("names a marked command and how it exited, while the shell runs on", () => {
		const mark = { command: "pnpm test", exit: 1, running: false };
		const lines = { first: 3, last: 4 };
		const ref = pickedOutput(terminal(), "Mac", { text: "1 failed", lines, mark }, 9);
		expect(ref).toMatchObject({ command: "pnpm test", exit_code: 1, running: false });
		// A command the shell didn't echo goes by the tab.
		const blank = { ...mark, command: "", exit: null, running: true };
		expect(pickedOutput(terminal(), "Mac", { text: "", lines, mark: blank }, 9)).toMatchObject({
			command: "zsh",
			exit_code: null,
			running: true,
		});
	});
});
