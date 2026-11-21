// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A terminal view for tests, which jsdom can't draw: it keeps what was
// written and pasted, and lets a test type, pick text, focus it, drop its
// GPU context, mark commands and click their marks.

import type { CommandOutput, MarkedCommand, Picked, TermView } from "@/terminal/term-view";

/** A command a test marks, with what it printed and, when it says, where its mark starts. */
type FakeCommand = Omit<MarkedCommand, "offset"> & {
	readonly offset?: number | null;
	readonly output: CommandOutput;
};

export type FakeView = TermView & {
	readonly written: string[];
	readonly cleared: () => number;
	readonly gpu: () => boolean;
	readonly type: (data: string) => void;
	readonly pick: (picked: Picked | null) => void;
	readonly takeFocus: (focused: boolean) => void;
	readonly loseGpu: () => void;
	readonly disposed: () => boolean;
	readonly pasted: string[];
	/** Sets the commands it marked, with what each printed. */
	readonly mark: (commands: readonly FakeCommand[]) => void;
	/** Clicks a command's mark, by `anchor`. */
	readonly clickMark: (id: number, anchor: Element) => void;
	/** Each jump asked for, true going back. */
	readonly jumps: boolean[];
	/** Whether it was told to take its shell's marks. */
	readonly takesMarks: () => boolean;
};

export const fakeView = (): FakeView => {
	const element = document.createElement("div");
	const written: string[] = [];
	const pasted: string[] = [];
	const data = new Set<(data: string) => void>();
	const focus = new Set<(focused: boolean) => void>();
	const lost = new Set<() => void>();
	const picks = new Set<() => void>();
	const marks = new Set<(id: number, anchor: Element) => void>();
	const jumps: boolean[] = [];
	let commands: readonly FakeCommand[] = [];
	let cleared = 0;
	let gpu = false;
	let takesMarks = false;
	let disposed = false;
	let picked: Picked | null = null;
	let size = { cols: 80, rows: 24 };
	const decoder = new TextDecoder();
	const text = () => written.join("");
	return {
		element,
		written,
		pasted,
		cleared: () => cleared,
		gpu: () => gpu,
		disposed: () => disposed,
		type: (typed) => {
			for (const listener of data) listener(typed);
		},
		pick: (next) => {
			picked = next;
			for (const listener of picks) listener();
		},
		takeFocus: (focused) => {
			for (const listener of focus) listener(focused);
		},
		loseGpu: () => {
			gpu = false;
			for (const listener of lost) listener();
		},
		mark: (next) => {
			commands = next;
		},
		clickMark: (id, anchor) => {
			for (const listener of marks) listener(id, anchor);
		},
		jumps,
		takesMarks: () => takesMarks,
		write: (bytes, _offset, done) => {
			written.push(decoder.decode(bytes));
			done?.();
		},
		clear: () => {
			cleared++;
			written.length = 0;
		},
		resize: (cols, rows) => {
			size = { cols, rows };
		},
		fit: () => ({ cols: 100, rows: 30 }),
		size: () => size,
		usedRows: () => text().split("\n").length,
		focus: () => {},
		setInteractive: () => {},
		selection: () => picked,
		clearSelection: () => {
			picked = null;
		},
		text: () => ({ text: text(), total: text().split("\n").length }),
		paste: (pasting) => pasted.push(pasting),
		find: () => false,
		setGpu: (on) => {
			gpu = on;
		},
		onData: (listener) => {
			data.add(listener);
			return () => data.delete(listener);
		},
		onTitle: () => () => {},
		onSelection: (listener) => {
			picks.add(listener);
			return () => picks.delete(listener);
		},
		onFocus: (listener) => {
			focus.add(listener);
			return () => focus.delete(listener);
		},
		onGpuLost: (listener) => {
			lost.add(listener);
			return () => lost.delete(listener);
		},
		setMarks: (on) => {
			takesMarks = on;
		},
		commands: () =>
			commands.map(({ id, command, exit, running, offset = null }) => ({
				id,
				command,
				exit,
				running,
				offset,
			})),
		output: (id) => commands.find((command) => command.id === id)?.output ?? null,
		jump: (back) => {
			jumps.push(back);
			return commands.length > 0;
		},
		onMark: (listener) => {
			marks.add(listener);
			return () => marks.delete(listener);
		},
		dispose: () => {
			disposed = true;
			element.remove();
		},
	};
};
