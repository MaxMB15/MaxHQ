// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Terminal } from "../bindings";
import { agent, entities, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { pickedOutput, runningCommand, runningCommands, tabTitle } from "./drawer";
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
	password_prompt: false,
	started_at: NOW - 10 * MIN,
	cols: 120,
	rows: 32,
	...over,
});

describe("runningCommand", () => {
	it("reads a shell at its prompt as waiting, whatever title it sets there", () => {
		for (const title of [
			"zsh",
			"-bash",
			"/bin/fish",
			"~/code/example-app",
			"you@devbox: ~",
			"",
		])
			expect(runningCommand(terminal({ title }))).toBeNull();
	});

	it("names what a shell runs, and what a command you ran runs", () => {
		expect(runningCommand(terminal({ title: "pnpm dev" }))).toBe("pnpm dev");
		const run = terminal({
			kind: { type: "run", command: "pnpm test", private: false },
			title: "vitest",
		});
		expect(runningCommand(run)).toBe("pnpm test");
	});

	it("names nothing once it exits, or for a name you gave the tab", () => {
		const exited = terminal({
			title: "pnpm dev",
			status: { state: "exited", code: 0, signal: null },
		});
		expect(runningCommand(exited)).toBeNull();
		expect(runningCommand(terminal({ title: "server" }), "server")).toBeNull();
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
			terminal({ id: "t-dev", agent: "lead", title: "pnpm dev" }),
			terminal({ id: "t-idle", agent: "lead", title: "zsh" }),
			terminal({
				id: "t-sub",
				agent: "sub",
				kind: { type: "run", command: "pnpm test", private: false },
			}),
			terminal({ id: "t-again", agent: "sub", title: "pnpm dev" }),
			terminal({ id: "t-named", agent: "lead", title: "server" }),
			terminal({ id: "t-other", agent: "other", title: "cargo watch" }),
		],
	});

	it("names each command still running in the agents' and their sub-agents' terminals once", () => {
		expect(runningCommands(view, new Set(["lead"]))).toEqual([
			"pnpm dev",
			"pnpm test",
			"server",
		]);
		expect(runningCommands(view, new Set(["lead"]), { "t-named": "server" })).toEqual([
			"pnpm dev",
			"pnpm test",
		]);
		expect(runningCommands(view, new Set(["gone"]))).toEqual([]);
	});
});

describe("pickedOutput", () => {
	it("makes an Output pill with the picked lines and the terminal it came from", () => {
		const picked = { text: "served stale=true\nshowing banner", lines: { first: 6, last: 7 } };
		expect(pickedOutput(terminal({ title: "pnpm dev" }), "Mac", picked, 9)).toEqual({
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
		});
	});

	it("says how a finished terminal exited, and names a shell by its tab", () => {
		const done = terminal({ status: { state: "exited", code: 1, signal: null } });
		const ref = pickedOutput(done, "devbox", { text: "x", lines: { first: 2, last: 2 } }, 1);
		expect(ref).toMatchObject({ command: "zsh", exit_code: 1, running: false, lines_total: 2 });
		expect(tabTitle(terminal({ title: "" }))).toBe("example-app");
	});
});
