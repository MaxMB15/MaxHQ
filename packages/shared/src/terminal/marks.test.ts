// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { type CommandSpots, readMark, type ShellMark, takeMark } from "./marks";

describe("readMark", () => {
	it("reads the four marks and how a command exited", () => {
		expect(readMark("A")).toEqual({ kind: "prompt" });
		expect(readMark("B")).toEqual({ kind: "input" });
		expect(readMark("C")).toEqual({ kind: "command" });
		expect(readMark("D;0")).toEqual({ kind: "done", exit: 0 });
		expect(readMark("D;-1")).toEqual({ kind: "done", exit: -1 });
	});

	it("reads an end with no number as one that didn't say", () => {
		expect(readMark("D")).toEqual({ kind: "done", exit: null });
		expect(readMark("D;x")).toEqual({ kind: "done", exit: null });
	});

	it("passes over marks the drawer doesn't use", () => {
		for (const body of ["", "E;ls", "P;k=i", "a"]) expect(readMark(body)).toBeNull();
	});
});

type Command = CommandSpots<number> & { id: number };

describe("takeMark", () => {
	const start = () => {
		const list: Command[] = [];
		let id = 0;
		const fresh = (): Command => ({
			id: ++id,
			prompt: null,
			input: null,
			start: null,
			end: null,
			exit: null,
		});
		const take = (mark: ShellMark, at: number) => takeMark(list, mark, at, fresh);
		return { list, take };
	};

	it("places one command from its prompt to its end", () => {
		const { list, take } = start();
		expect(take({ kind: "prompt" }, 1)).toEqual({
			kept: true,
			dropped: null,
			ended: null,
			started: null,
		});
		expect(take({ kind: "input" }, 2).kept).toBe(true);
		const started = take({ kind: "command" }, 3);
		expect(started.started).toBe(list[0]);
		const ended = take({ kind: "done", exit: 2 }, 4);
		expect(ended.ended).toBe(list[0]);
		expect(list).toEqual([{ id: 1, prompt: 1, input: 2, start: 3, end: 4, exit: 2 }]);
	});

	it("lets go of a prompt nothing ran from", () => {
		const { list, take } = start();
		take({ kind: "prompt" }, 1);
		take({ kind: "input" }, 2);
		const next = take({ kind: "prompt" }, 3);
		expect(next.dropped).toMatchObject({ id: 1, prompt: 1, input: 2 });
		expect(list).toEqual([
			{ id: 2, prompt: 3, input: null, start: null, end: null, exit: null },
		]);
	});

	it("ends a command that never said so at the next prompt or command", () => {
		const { list, take } = start();
		take({ kind: "prompt" }, 1);
		take({ kind: "command" }, 2);
		const prompt = take({ kind: "prompt" }, 3);
		expect(prompt.ended).toBe(list[0]);
		expect(list[0]).toMatchObject({ start: 2, end: 3, exit: null });

		take({ kind: "command" }, 4);
		const inner = take({ kind: "command" }, 5);
		expect(inner.ended).toBe(list[1]);
		expect(inner.started).toBe(list[2]);
		expect(list.map((command) => [command.start, command.end])).toEqual([
			[2, 3],
			[4, 5],
			[5, null],
		]);
	});

	it("keeps no place for a mark that fits nothing", () => {
		const { list, take } = start();
		// An end with nothing running, and input with no prompt waiting.
		expect(take({ kind: "done", exit: 0 }, 1).kept).toBe(false);
		take({ kind: "prompt" }, 2);
		take({ kind: "command" }, 3);
		expect(take({ kind: "input" }, 4).kept).toBe(false);
		take({ kind: "done", exit: 0 }, 5);
		expect(take({ kind: "done", exit: 1 }, 6).kept).toBe(false);
		expect(list).toEqual([{ id: 1, prompt: 2, input: null, start: 3, end: 5, exit: 0 }]);
	});
});
