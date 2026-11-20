// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The view's shell marks on a real xterm. jsdom can't open one, so the
// terminal stays unopened, which still parses and keeps its buffer, and
// each mark's element is handed in where xterm would draw it.

import { type IDecoration, type IDecorationOptions, Terminal } from "@xterm/xterm";
import { afterEach, describe, expect, it, vi } from "vitest";

import { commandText } from "./term-view";
import { xtermView } from "./term-view-xterm";

const encode = (text: string) => new TextEncoder().encode(text);

const mark = (body: string) => `\x1b]133;${body}\x07`;

/** A prompt, a command typed at it and what it printed, ending how `exit` says. */
const ran = (command: string, output: string, exit: number | null) =>
	`${mark("A")}$ ${mark("B")}${command}\r\n${mark("C")}${output}` +
	(exit === null ? "" : mark(`D;${exit}`));

const make = ({ marks = true } = {}) => {
	const loads = vi.spyOn(Terminal.prototype, "loadAddon");
	const real = Terminal.prototype.registerDecoration;
	// Each mark's draw call, as xterm would make it once the line shows.
	const draws: ((element: HTMLElement) => void)[] = [];
	const decorations: IDecoration[] = [];
	vi.spyOn(Terminal.prototype, "registerDecoration").mockImplementation(function (
		this: Terminal,
		options: IDecorationOptions
	) {
		const decoration = real.call(this, options);
		if (!decoration) return decoration;
		const onRender = decoration.onRender;
		Object.defineProperty(decoration, "onRender", {
			value: (listener: (element: HTMLElement) => void) => {
				draws.push(listener);
				return onRender(listener);
			},
		});
		decorations.push(decoration);
		return decoration;
	});
	const view = xtermView();
	view.setMarks(marks);
	const terminal = loads.mock.contexts[0] as Terminal;
	const scrolls = vi.spyOn(terminal, "scrollToLine");
	/** Prints `text` and waits for xterm to take it in. */
	const print = async (text: string) => {
		view.write(encode(text));
		await new Promise<void>((done) => terminal.write("", done));
	};
	/** Draws every mark so far, each in its own element, and gives their dots. */
	const drawAll = () =>
		draws.map((draw) => {
			const element = document.createElement("div");
			draw(element);
			return element.firstElementChild as HTMLElement;
		});
	return { view, terminal, print, drawAll, decorations, scrolls };
};

afterEach(() => {
	vi.restoreAllMocks();
});

describe("a terminal view whose shell marks its commands", () => {
	it("lists each command with what it printed and how it ended", async () => {
		const { view, print } = make();
		await print(ran("pnpm test", "ok\r\n1 failed\r\n", 1) + ran("sleep 9", "zzz", null));

		const [failed, running] = view.commands();
		expect(failed).toEqual({
			id: expect.any(Number),
			command: "pnpm test",
			exit: 1,
			running: false,
			offset: null,
		});
		expect(running).toMatchObject({ command: "sleep 9", exit: null, running: true });
		expect(view.output(failed!.id)).toEqual({
			text: "ok\n1 failed",
			lines: { first: 2, last: 3 },
			mark: { command: "pnpm test", exit: 1, running: false },
		});
		// A running command's output runs to the cursor.
		expect(view.output(running!.id)).toMatchObject({
			text: "zzz",
			lines: { first: 5, last: 5 },
			mark: { running: true },
		});
		expect(view.output(999)).toBeNull();
	});

	it("gives an empty output for a command that printed nothing", async () => {
		const { view, print } = make();
		await print(ran("true", "", 0) + `${mark("A")}$ `);
		const [quiet] = view.commands();
		expect(view.output(quiet!.id)).toEqual({
			text: "",
			lines: { first: 2, last: 2 },
			mark: { command: "true", exit: 0, running: false },
		});
	});

	it("drops a prompt nothing ran from and keeps no command for it", async () => {
		const { view, print, decorations } = make();
		await print(`${mark("A")}$ ${mark("B")}\r\n${mark("A")}$ ${mark("B")}`);
		expect(view.commands()).toEqual([]);
		expect(decorations).toEqual([]);
	});

	it("draws a mark at each command that shows how it stands and opens its menu", async () => {
		const { view, print, drawAll } = make();
		const opened = vi.fn();
		view.onMark(opened);
		await print(ran("pnpm build", "done\r\n", 0) + ran("pnpm test", "", null));

		const [built, testing] = drawAll();
		expect(built!.dataset.state).toBe("done");
		expect(built!.title).toBe("pnpm build");
		expect(testing!.dataset.state).toBe("running");

		await print(mark("D;2"));
		expect(testing!.dataset.state).toBe("failed");

		testing!.click();
		expect(opened).toHaveBeenCalledWith(view.commands()[1]!.id, testing);
	});

	it("jumps up and down between commands and shows the one it picked", async () => {
		const { view, terminal, print, drawAll, scrolls } = make();
		view.resize(40, 4);
		await print(
			ran("one", "a\r\nb\r\n", 0) +
				ran("two", "c\r\nd\r\n", 0) +
				ran("three", "e\r\nf\r\n", 0)
		);
		await print(`${mark("A")}$ `);
		const dots = drawAll();
		const lines = view.commands().map((command) => command.id);
		expect(lines).toHaveLength(3);

		// From the end, up goes to the last command, then the one before.
		expect(view.jump(true)).toBe(true);
		expect(scrolls).toHaveBeenLastCalledWith(6);
		expect(dots[2]!.hasAttribute("data-current")).toBe(true);
		expect(view.jump(true)).toBe(true);
		expect(scrolls).toHaveBeenLastCalledWith(3);
		expect(dots[2]!.hasAttribute("data-current")).toBe(false);
		expect(dots[1]!.hasAttribute("data-current")).toBe(true);

		// Down goes back, and past the last command to the end.
		expect(view.jump(false)).toBe(true);
		expect(scrolls).toHaveBeenLastCalledWith(6);
		const bottom = vi.spyOn(terminal, "scrollToBottom");
		expect(view.jump(false)).toBe(false);
		expect(bottom).toHaveBeenCalled();
		expect(dots.some((dot) => dot.hasAttribute("data-current"))).toBe(false);
	});

	it("jumps to a command's output once the scrollback dropped its prompt", async () => {
		const { view, terminal, print, decorations, scrolls } = make();
		view.resize(40, 4);
		terminal.options.scrollback = 4;
		await print(ran("one", "a\r\n", 0) + ran("two", "b\r\n", 0) + `${mark("A")}$ `);
		// Only the first command's prompt line goes.
		for (let lines = 0; !decorations[0]!.isDisposed && lines < 50; lines++) await print("\r\n");
		expect(decorations[0]!.isDisposed).toBe(true);
		expect(view.commands().map((command) => command.command)).toEqual(["one", "two"]);

		expect(view.jump(true)).toBe(true);
		expect(view.jump(true)).toBe(true);
		// Its output's first line is now the buffer's first.
		expect(scrolls).toHaveBeenLastCalledWith(0);
		expect(view.output(view.commands()[0]!.id)?.text).toBe("a");
	});

	it("leaves a right prompt out of what was typed", async () => {
		const { view, print } = make();
		view.resize(40, 4);
		// zsh draws the right prompt after the left, then goes back to type.
		const right = (command: string) =>
			`${mark("A")}$ ${mark("B")}\x1b[36Gmain\x1b[3G${command}\r\n${mark("C")}ok\r\n${mark("D;0")}`;
		await print(right("pnpm test") + right("echo a    b") + ran("echo c    d", "", 0));
		expect(view.commands().map((command) => command.command)).toEqual([
			"pnpm test",
			"echo a    b",
			"echo c    d",
		]);
	});

	it("starts over on clear and takes the marks back from a replay", async () => {
		const { view, print, decorations } = make();
		const bytes = ran("pnpm test", "1 failed\r\n", 1);
		await print(bytes);
		expect(view.commands()).toHaveLength(1);

		view.clear();
		expect(view.commands()).toEqual([]);
		expect(decorations.every((decoration) => decoration.isDisposed)).toBe(true);

		await print(bytes);
		expect(view.commands()).toEqual([
			{ id: expect.any(Number), command: "pnpm test", exit: 1, running: false, offset: null },
		]);
	});

	it("takes no marks, and shows none, unless its holder keeps to the shell's own", async () => {
		const { view, print, decorations } = make({ marks: false });
		await print(ran("pnpm test", "ok\r\n", 0) + mark("D;1;ignored"));
		expect(view.commands()).toEqual([]);
		expect(decorations).toEqual([]);
		expect(view.text(10).text).toBe("$ pnpm test\nok");

		view.setMarks(true);
		await print(ran("ls", "", 0));
		expect(view.commands()).toMatchObject([{ command: "ls", exit: 0 }]);
	});

	it("knows where each command's mark starts, even split across writes", async () => {
		const { view, terminal } = make();
		// A shell started inside a command marks its own commands after it.
		const outer = ran("nix develop", "entering\r\n", null);
		const inner = ran("pnpm test", "ok\r\n", null);
		const bytes = encode(outer + inner);
		const cut = outer.length + inner.indexOf(mark("C")) + 3;
		view.write(bytes.subarray(0, cut), 100);
		view.write(bytes.subarray(cut), 100 + cut);
		await new Promise<void>((done) => terminal.write("", done));

		const outerStart = 100 + outer.indexOf(mark("C"));
		const innerStart = 100 + outer.length + inner.indexOf(mark("C"));
		expect(view.commands().map(({ command, offset }) => [command, offset])).toEqual([
			["nix develop", outerStart],
			["pnpm test", innerStart],
		]);
		// The outer command's text is its own, not the nested shell's last command.
		expect(commandText(view, outerStart)).toEqual({ text: "entering", total: 1 });
		expect(commandText(view, innerStart)).toEqual({ text: "ok", total: 1 });
		expect(commandText(view, innerStart + 1)).toBeNull();
	});

	it("works as before for a shell that marks nothing", async () => {
		const { view, print, scrolls } = make();
		await print("$ ls\r\nREADME.md\r\n$ ");
		expect(view.commands()).toEqual([]);
		expect(view.output(1)).toBeNull();
		expect(view.jump(true)).toBe(false);
		expect(scrolls).not.toHaveBeenCalled();
		expect(view.text(10).text).toBe("$ ls\nREADME.md\n$");
	});
});
