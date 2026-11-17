// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A terminal view for tests, which jsdom can't draw: it keeps what was
// written and pasted, and lets a test type, pick text, focus it and drop
// its GPU context.

import type { Picked, TermView } from "@/terminal/term-view";

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
};

export const fakeView = (): FakeView => {
	const element = document.createElement("div");
	const written: string[] = [];
	const pasted: string[] = [];
	const data = new Set<(data: string) => void>();
	const focus = new Set<(focused: boolean) => void>();
	const lost = new Set<() => void>();
	const picks = new Set<() => void>();
	let cleared = 0;
	let gpu = false;
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
		write: (bytes) => written.push(decoder.decode(bytes)),
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
		dispose: () => {
			disposed = true;
			element.remove();
		},
	};
};
