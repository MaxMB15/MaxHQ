// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Terminal } from "@xterm/xterm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { type FakeView, fakeView } from "@/test/fake-term-view";
import type { TerminalId } from "@maxhq/shared/bindings";
import type { TerminalEvent, TerminalStream } from "@maxhq/shared/terminal/stream";

import { createRegistry } from "./registry";
import { xtermView } from "./term-view-xterm";

const fakeStream = () => {
	const listeners = new Map<TerminalId, Set<(event: TerminalEvent) => void>>();
	const stream = {
		attach: vi.fn<TerminalStream["attach"]>(() => Promise.resolve()),
		detach: vi.fn(),
		resize: vi.fn(),
		input: vi.fn(),
		listen: (terminal: TerminalId, listener: (event: TerminalEvent) => void) => {
			const set = listeners.get(terminal) ?? new Set();
			set.add(listener);
			listeners.set(terminal, set);
			return () => set.delete(listener);
		},
	} satisfies TerminalStream;
	const emit = (terminal: TerminalId, event: TerminalEvent) => {
		for (const listener of listeners.get(terminal) ?? []) listener(event);
	};
	const output = (terminal: TerminalId, offset: number, text: string) =>
		emit(terminal, { type: "output", offset, bytes: new TextEncoder().encode(text) });
	return { stream, emit, output };
};

const setup = (
	over: {
		exists?: (terminal: TerminalId) => boolean;
		marksOf?: (terminal: TerminalId) => boolean;
		gpuBudget?: number;
	} = {}
) => {
	const { stream, emit, output } = fakeStream();
	const views: FakeView[] = [];
	const registry = createRegistry({
		stream,
		makeView: () => {
			const view = fakeView();
			views.push(view);
			return view;
		},
		sizeOf: () => ({ cols: 120, rows: 32 }),
		exists: over.exists ?? (() => true),
		marksOf: over.marksOf,
		detachAfter: 30_000,
		gpuBudget: over.gpuBudget,
	});
	return { registry, stream, emit, output, views };
};

const host = () => {
	const element = document.createElement("div");
	document.body.append(element);
	return element;
};

describe("the terminal registry", () => {
	beforeEach(() => {
		vi.useFakeTimers();
	});
	afterEach(() => {
		vi.useRealTimers();
		document.body.replaceChildren();
	});

	it("moves one view between hosts and gives it back, attaching once", () => {
		const { registry, stream, views } = setup();
		const drawer = host();
		const pane = host();
		const leaveDrawer = registry.place("t-1", drawer, { interactive: true });
		const leavePane = registry.place("t-1", pane, { interactive: true });
		expect(views).toHaveLength(1);
		expect(pane.contains(views[0]!.element)).toBe(true);
		expect(stream.attach).toHaveBeenCalledTimes(1);
		expect(stream.attach).toHaveBeenCalledWith("t-1", null);
		leavePane();
		expect(drawer.contains(views[0]!.element)).toBe(true);
		leaveDrawer();
		expect(views[0]!.element.isConnected).toBe(false);
		expect(views[0]!.disposed()).toBe(false);
	});

	it("detaches a while after the last place goes, and attaches again from where it ended", () => {
		const { registry, stream, output, views } = setup();
		const leave = registry.place("t-1", host(), { interactive: true });
		output("t-1", 0, "$ ls\n");
		leave();
		vi.advanceTimersByTime(29_000);
		expect(stream.detach).not.toHaveBeenCalled();
		// Coming back in time keeps it attached.
		const again = registry.place("t-1", host(), { interactive: true });
		vi.advanceTimersByTime(60_000);
		expect(stream.detach).not.toHaveBeenCalled();
		again();
		vi.advanceTimersByTime(30_000);
		expect(stream.detach).toHaveBeenCalledWith("t-1");
		registry.place("t-1", host(), { interactive: true });
		// "$ ls\n" is bytes 0 to 4, so it goes on after byte 4.
		expect(stream.attach).toHaveBeenLastCalledWith("t-1", 4);
		expect(views).toHaveLength(1);
		expect(views[0]!.written.join("")).toBe("$ ls\n");
	});

	it("skips what it shows, and clears for a reset or a gap", () => {
		const { registry, emit, output, views } = setup();
		registry.place("t-1", host(), { interactive: true });
		output("t-1", 0, "one\n");
		output("t-1", 0, "one\ntwo\n");
		expect(views[0]!.written.join("")).toBe("one\ntwo\n");
		emit("t-1", { type: "reset", from: 100 });
		output("t-1", 100, "three\n");
		expect(views[0]!.cleared()).toBe(1);
		expect(views[0]!.written.join("")).toBe("three\n");
		output("t-1", 200, "four\n");
		expect(views[0]!.cleared()).toBe(2);
		expect(views[0]!.written.join("")).toBe("four\n");
	});

	it("lets a view take its shell's marks only as the device says", () => {
		const trusted = new Set<TerminalId>(["t-1"]);
		const { registry, output, views } = setup({ marksOf: (terminal) => trusted.has(terminal) });
		registry.place("t-1", host(), { interactive: true });
		registry.place("t-2", host(), { interactive: true });
		output("t-1", 0, "$ ");
		output("t-2", 0, "$ ");
		expect(views.map((view) => view.takesMarks())).toEqual([true, false]);
		trusted.delete("t-1");
		output("t-1", 2, "ls\n");
		expect(views[0]!.takesMarks()).toBe(false);
	});

	it("drops the view of a terminal that's gone, instead of only detaching", () => {
		let there = true;
		const { registry, stream, views } = setup({ exists: () => there });
		registry.place("t-1", host(), { interactive: true })();
		there = false;
		vi.advanceTimersByTime(30_000);
		expect(stream.detach).toHaveBeenCalledWith("t-1");
		expect(views[0]!.disposed()).toBe(true);
		expect(registry.view("t-1")).toBeNull();
	});

	it("sends typing and the size only from a place that takes keys", () => {
		const { registry, stream, views } = setup();
		const leave = registry.place("t-1", host(), { interactive: true });
		expect(stream.resize).toHaveBeenCalledWith("t-1", { cols: 100, rows: 30 });
		views[0]!.type("ls\r");
		expect(new TextDecoder().decode(stream.input.mock.calls[0]![1])).toBe("ls\r");
		leave();
		registry.place("t-1", host(), { interactive: false, maxRows: 10 });
		views[0]!.type("x");
		expect(stream.input).toHaveBeenCalledTimes(1);
		expect(stream.resize).toHaveBeenCalledTimes(1);
		// A card takes the terminal's width and only the rows it uses.
		expect(views[0]!.size()).toEqual({ cols: 120, rows: 1 });
	});

	it("holds a paste until the terminal shows where it takes keys and printed its prompt", () => {
		const { registry, output, views } = setup();
		registry.paste("t-1", "git status");
		registry.place("t-1", host(), { interactive: true });
		expect(views).toHaveLength(1);
		expect(views[0]!.pasted).toEqual([]);
		output("t-1", 0, "$ ");
		expect(views[0]!.pasted).toEqual(["git status"]);
	});

	it("draws eight views with the GPU, the focused one first, and the rest with the DOM", () => {
		const { registry, views } = setup({ gpuBudget: 2 });
		registry.place("t-1", host(), { interactive: true });
		registry.place("t-2", host(), { interactive: true });
		registry.place("t-3", host(), { interactive: true });
		expect(views.map((view) => view.gpu())).toEqual([false, true, true]);
		views[0]!.takeFocus(true);
		expect(views.map((view) => view.gpu())).toEqual([true, false, true]);
		// A view that lost its context draws with the DOM and frees its place.
		views[0]!.loseGpu();
		expect(views.map((view) => view.gpu())).toEqual([false, true, true]);
		expect(registry.focused()).toBe("t-1");
	});

	it("takes the GPU from a view that no longer shows", () => {
		const { registry, views } = setup({ gpuBudget: 1 });
		const leave = registry.place("t-1", host(), { interactive: true });
		registry.place("t-2", host(), { interactive: true });
		expect(views.map((view) => view.gpu())).toEqual([false, true]);
		registry.forget("t-2");
		expect(views[1]!.disposed()).toBe(true);
		expect(views[0]!.gpu()).toBe(true);
		leave();
		expect(views[0]!.gpu()).toBe(false);
	});
});

describe("a card's terminal on a real xterm", () => {
	afterEach(() => {
		vi.restoreAllMocks();
	});

	// xterm takes writes in a moment later, so a card sized right after a
	// write showed the rows from before it, and a command's first line
	// didn't show until more came.
	it("shows each line and each redraw as it comes", async () => {
		const loads = vi.spyOn(Terminal.prototype, "loadAddon");
		const { stream, output } = fakeStream();
		const registry = createRegistry({
			stream,
			makeView: xtermView,
			sizeOf: () => ({ cols: 80, rows: 24 }),
			exists: () => true,
		});
		// jsdom can't open a terminal, so the host stays out of the page.
		registry.place("t-1", document.createElement("div"), {
			interactive: false,
			maxRows: 4,
		});
		const terminal = loads.mock.contexts[0] as Terminal;
		const shown = () => {
			const buffer = terminal.buffer.active;
			const rows: string[] = [];
			for (let y = buffer.viewportY; y < buffer.viewportY + terminal.rows; y++)
				rows.push(buffer.getLine(y)?.translateToString(true) ?? "");
			return rows;
		};
		let offset = 0;
		const print = async (text: string) => {
			output("t-1", offset, text);
			offset += new TextEncoder().encode(text).length;
			await new Promise<void>((done) => terminal.write("", done));
		};

		await print("1\r\n");
		expect(shown()).toEqual(["1", ""]);
		await print("2\r\n");
		expect(shown()).toEqual(["1", "2", ""]);
		await print("3\r\n4\r\n5\r\n");
		expect(shown()).toEqual(["3", "4", "5", ""]);
		// A progress bar redraws its line instead of adding lines.
		await print("\r10%");
		await print("\r20%");
		expect(shown()).toEqual(["3", "4", "5", "20%"]);
		registry.dispose();
	});
});
