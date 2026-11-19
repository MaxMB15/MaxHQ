// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The terminal view on xterm.js, the one file that imports it. It opens
// once its element first sits in the page, since xterm measures its font
// there, and keeps its screen and scrollback as the element moves between
// hosts. It draws with WebGL when the registry lets it, and falls back to
// the DOM when the GPU drops its context. Keys go to the shell, Control
// ones too, but for Command ones, which pass on to the window for the
// app's shortcuts and the drawer's find, and Command C and V, which copy
// and paste here. What you type leaves through `onData` and nothing here
// keeps it.
//
// A shell that marks its prompts and commands, OSC 133, gets a dot in the
// margin at each command's first line, which opens the command's menu.
// Marks sit on xterm markers, which move with the scrollback and go with
// the lines it drops.

import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { type IDecoration, type IMarker, Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import {
	type CommandMark,
	type CommandSpots,
	readMark,
	takeMark,
} from "@maxhq/shared/terminal/marks";

import { onMac } from "@/native/platform";

import { readPalette, watchPalette } from "./palette";
import type { MakeView, MarkedCommand, Picked, TermView } from "./term-view";

/** The app's code size. */
export const FONT_SIZE = 12;

/** The line height, as a multiple of the font size. */
export const LINE_HEIGHT = 1.25;

/** How many lines each terminal keeps above its screen. */
const SCROLLBACK = 10_000;

/** The space between the terminal's edge and its text. */
const PADDING = "6px 4px 4px 10px";

/** Only web addresses open from a terminal. */
const SAFE_LINK = /^https?:\/\//i;

let mono: string | null = null;

/** The app's monospace stack, as the `font-mono` class sets it. */
export const monoFamily = (): string => {
	if (mono !== null) return mono;
	const probe = document.createElement("span");
	probe.className = "font-mono";
	document.body.append(probe);
	mono = getComputedStyle(probe).fontFamily || "monospace";
	probe.remove();
	return mono;
};

/**
 * Whether a key goes to the app rather than the terminal: Command and any
 * key but C and V on a Mac, Control and Shift elsewhere, and Control and
 * the backquote, which shows and hides the drawer.
 */
export const keyForApp = (e: KeyboardEvent): boolean => {
	if (e.ctrlKey && !e.metaKey && e.code === "Backquote") return true;
	const command = onMac() ? e.metaKey : e.ctrlKey && e.shiftKey;
	if (!command) return false;
	// Copy and paste reach the terminal's own handlers as events.
	return !["KeyC", "KeyV"].includes(e.code);
};

const NEWLINE = 0x0a;

/** The OSC that marks a shell's prompts and commands. */
const OSC_MARKS = 133;

/** Where a mark sits: on its line, which moves with the scrollback, and in its column. */
type Spot = { readonly marker: IMarker; readonly x: number };

/** A command its shell marked, as the view holds it. */
type Held = CommandSpots<Spot> & {
	readonly id: number;
	/** What you typed, read when it starts. */
	command: string;
	/** Its mark in the margin, once it started. */
	decoration: IDecoration | null;
	/** The mark's element, once it drew. */
	dot: HTMLElement | null;
};

/** A row and a column in the buffer, with no column meaning the row's end or start. */
type Place = { readonly y: number; readonly x?: number };

// The mark fills the space left of the text, and its dot shows how the
// command stands: a ring while it runs, grey once it ended well and red
// once it failed.
const MARK =
	"group pointer-events-auto absolute top-0 -left-[10px] flex h-full w-[10px] cursor-pointer items-center justify-center";
const DOT =
	"size-[6px] rounded-full bg-subtle-foreground transition-transform group-hover:scale-150 group-data-current:scale-150 group-data-[state=failed]:bg-status-error group-data-[state=running]:bg-transparent group-data-[state=running]:ring-1 group-data-[state=running]:ring-subtle-foreground";

/** A terminal view on xterm.js. */
export const xtermView: MakeView = () => {
	const element = document.createElement("div");
	element.className = "h-full w-full overflow-hidden";
	const terminal = new Terminal({
		allowProposedApi: true,
		cursorBlink: false,
		cursorInactiveStyle: "outline",
		fontFamily: monoFamily(),
		fontSize: FONT_SIZE,
		lineHeight: LINE_HEIGHT,
		scrollback: SCROLLBACK,
		theme: readPalette(),
		macOptionIsMeta: false,
		macOptionClickForcesSelection: true,
		rightClickSelectsWord: false,
	});
	const fit = new FitAddon();
	const search = new SearchAddon();
	terminal.loadAddon(fit);
	terminal.loadAddon(search);
	terminal.loadAddon(new Unicode11Addon());
	terminal.unicode.activeVersion = "11";
	terminal.loadAddon(
		new WebLinksAddon((event, uri) => {
			if (!SAFE_LINK.test(uri)) return;
			event.preventDefault();
			window.open(uri, "_blank", "noreferrer");
		})
	);
	terminal.attachCustomKeyEventHandler((e) => !keyForApp(e));

	let opened = false;
	let gpu: WebglAddon | null = null;
	let wantGpu = false;
	let lost = false;
	// Lines counted since the view last started over, for numbering lines.
	let newlines = 0;
	const gpuLost = new Set<() => void>();
	const focusListeners = new Set<(focused: boolean) => void>();
	const markListeners = new Set<(id: number, anchor: Element) => void>();
	// The commands the shell marked, oldest first, and the one a jump showed last.
	let held: Held[] = [];
	let shown: Held | null = null;
	let lastId = 0;

	const stopPalette = watchPalette((palette) => {
		terminal.options.theme = palette;
	});

	const dropGpu = () => {
		gpu?.dispose();
		gpu = null;
	};

	const applyGpu = () => {
		if (!opened) return;
		if (!wantGpu || lost) return dropGpu();
		if (gpu) return;
		try {
			const addon = new WebglAddon();
			addon.onContextLoss(() => {
				lost = true;
				dropGpu();
				for (const listener of [...gpuLost]) listener();
			});
			terminal.loadAddon(addon);
			gpu = addon;
		} catch {
			// No WebGL here, so it draws with the DOM from now on.
			lost = true;
			for (const listener of [...gpuLost]) listener();
		}
	};

	/**
	 * Opens the terminal once its element is in the page, which the registry
	 * sees to before it sizes or focuses it. Output written before then waits
	 * in the terminal's buffer.
	 */
	const openIfPlaced = () => {
		if (opened || !element.isConnected) return;
		opened = true;
		terminal.open(element);
		if (terminal.element) terminal.element.style.padding = PADDING;
		const textarea = terminal.textarea;
		textarea?.addEventListener("focus", () => {
			for (const listener of [...focusListeners]) listener(true);
		});
		textarea?.addEventListener("blur", () => {
			for (const listener of [...focusListeners]) listener(false);
		});
		applyGpu();
	};

	const buffer = () => terminal.buffer.active;

	/** The row the output ends on, counting the scrollback. */
	const lastRow = () => buffer().baseY + buffer().cursorY;

	/** How many lines, not rows, start at or before `row`. */
	const linesThrough = (row: number) => {
		let lines = 0;
		for (let y = 0; y <= row; y++) {
			if (!buffer().getLine(y)?.isWrapped) lines++;
		}
		return lines;
	};

	/** The number, in all it printed, of the buffer's first line. */
	const firstNumber = () => Math.max(1, newlines + 1 - linesThrough(lastRow()) + 1);

	/** The number, in all it printed, of the line `row` is on. */
	const numberOf = (row: number) => firstNumber() + linesThrough(row) - 1;

	/** The text from one place to another, with each row a wrap continues joined to its line. */
	const textBetween = (from: Place, to: Place): string => {
		const lines: string[] = [];
		for (let y = from.y; y <= to.y; y++) {
			const line = buffer().getLine(y);
			// A row a wrap continues keeps its spaces, which the wrap may need.
			const wraps = y < to.y && buffer().getLine(y + 1)?.isWrapped === true;
			const start = y === from.y ? (from.x ?? 0) : 0;
			const text =
				line?.translateToString(!wraps, start, y === to.y ? to.x : undefined) ?? "";
			if (y > from.y && line?.isWrapped) lines.push(`${lines.pop() ?? ""}${text}`);
			else lines.push(text);
		}
		return lines.join("\n");
	};

	const placeOf = (spot: Spot): Place => ({ y: spot.marker.line, x: spot.x });

	/** The commands that ran, while the scrollback still holds where they start. */
	const ran = () => held.filter((command) => command.start && !command.start.marker.isDisposed);

	/** The line a command's mark sits on. */
	const lineOf = (command: Held) =>
		(command.prompt ?? command.input ?? command.start)?.marker.line ?? -1;

	const markOf = (command: Held): CommandMark => ({
		command: command.command,
		exit: command.exit,
		running: command.start !== null && command.end === null,
	});

	/** Shows how a command stands on its mark. */
	const paint = (command: Held) => {
		const dot = command.dot;
		if (!dot) return;
		const { running, exit } = markOf(command);
		dot.dataset.state = running ? "running" : exit ? "failed" : "done";
		dot.toggleAttribute("data-current", command === shown);
		dot.title = command.command;
	};

	/** Draws a command's mark in the element xterm keeps for it, once. */
	const draw = (command: Held, element: HTMLElement) => {
		if (command.dot?.parentElement === element) return;
		// Text under the element stays yours to pick.
		element.style.pointerEvents = "none";
		const dot = document.createElement("div");
		dot.className = MARK;
		dot.append(Object.assign(document.createElement("span"), { className: DOT }));
		// A press on the mark doesn't start picking text.
		dot.addEventListener("mousedown", (event) => {
			event.preventDefault();
			event.stopPropagation();
		});
		dot.addEventListener("click", (event) => {
			event.stopPropagation();
			for (const listener of [...markListeners]) listener(command.id, dot);
		});
		element.replaceChildren(dot);
		command.dot = dot;
		paint(command);
	};

	/** Reads what you typed and puts the command's mark in the margin. */
	const begin = (command: Held) => {
		const { input, start } = command;
		command.command = input && start ? textBetween(placeOf(input), placeOf(start)).trim() : "";
		const anchor = command.prompt ?? input ?? start;
		if (!anchor) return;
		command.decoration = terminal.registerDecoration({ marker: anchor.marker }) ?? null;
		command.decoration?.onRender((element) => draw(command, element));
	};

	/** Lets go of a command's mark and places, but for `keep`, which another holds. */
	const forget = (command: Held, keep: Spot | null = null) => {
		command.decoration?.dispose();
		for (const spot of [command.prompt, command.input, command.start, command.end]) {
			if (spot !== keep) spot?.marker.dispose();
		}
	};

	terminal.parser.registerOscHandler(OSC_MARKS, (body) => {
		const mark = readMark(body);
		if (!mark) return false;
		const at: Spot = { marker: terminal.registerMarker(0), x: buffer().cursorX };
		const fresh = (): Held => ({
			id: ++lastId,
			prompt: null,
			input: null,
			start: null,
			end: null,
			exit: null,
			command: "",
			decoration: null,
			dot: null,
		});
		// Commands whose start the scrollback dropped go, oldest first.
		while (held[0] && (held[0].start ?? held[0].prompt)?.marker.isDisposed) {
			forget(held.shift()!);
		}
		const change = takeMark(held, mark, at, fresh);
		if (!change.kept) at.marker.dispose();
		// The prompt it lets go of may be where the command before it ended.
		if (change.dropped) forget(change.dropped, held.at(-2)?.end ?? null);
		if (change.started) begin(change.started);
		if (change.ended) paint(change.ended);
		return true;
	});

	const listen =
		<T>(event: (listener: (value: T) => void) => { dispose: () => void }) =>
		(listener: (value: T) => void) => {
			const subscription = event(listener);
			return () => subscription.dispose();
		};

	const view: TermView = {
		element,
		write: (bytes) => {
			for (const byte of bytes) if (byte === NEWLINE) newlines++;
			terminal.write(bytes);
		},
		clear: () => {
			newlines = 0;
			for (const command of held) forget(command);
			held = [];
			shown = null;
			terminal.reset();
		},
		resize: (cols, rows) => {
			openIfPlaced();
			if (cols > 0 && rows > 0 && (cols !== terminal.cols || rows !== terminal.rows))
				terminal.resize(cols, rows);
		},
		fit: () => {
			openIfPlaced();
			if (!opened) return null;
			const size = fit.proposeDimensions();
			if (!size || !Number.isFinite(size.cols) || !Number.isFinite(size.rows)) return null;
			const cols = Math.max(2, size.cols);
			const rows = Math.max(1, size.rows);
			view.resize(cols, rows);
			return { cols, rows };
		},
		size: () => ({ cols: terminal.cols, rows: terminal.rows }),
		usedRows: () => lastRow() + 1,
		focus: () => {
			openIfPlaced();
			terminal.focus();
		},
		setInteractive: (interactive) => {
			openIfPlaced();
			terminal.options.disableStdin = !interactive;
			terminal.options.cursorInactiveStyle = interactive ? "outline" : "none";
		},
		selection: () => {
			const range = terminal.getSelectionPosition();
			const text = terminal.getSelection();
			if (!range || !text.trim()) return null;
			const first = firstNumber();
			const lines = {
				first: first + linesThrough(range.start.y) - 1,
				last: first + linesThrough(range.end.y) - 1,
			};
			return { text, lines } satisfies Picked;
		},
		clearSelection: () => terminal.clearSelection(),
		text: (count) => {
			const last = lastRow();
			// The row the first of the last `count` lines starts on.
			let from = last + 1;
			for (let lines = 0; lines < count && from > 0;) {
				from -= 1;
				if (!buffer().getLine(from)?.isWrapped) lines += 1;
			}
			const text = textBetween({ y: from }, { y: last }).replace(/\s+$/, "");
			return { text, total: numberOf(last) };
		},
		paste: (text) => {
			terminal.paste(text);
		},
		find: (query, back) =>
			query !== "" && (back ? search.findPrevious(query) : search.findNext(query)),
		setGpu: (on) => {
			wantGpu = on;
			applyGpu();
		},
		onData: (listener) => {
			const data = terminal.onData(listener);
			// Keys such as a mouse report in an old mode come as bytes.
			const binary = terminal.onBinary(listener);
			return () => {
				data.dispose();
				binary.dispose();
			};
		},
		onTitle: listen((listener) => terminal.onTitleChange(listener)),
		onSelection: (listener) => {
			const subscription = terminal.onSelectionChange(listener);
			return () => subscription.dispose();
		},
		onFocus: (listener) => {
			focusListeners.add(listener);
			return () => focusListeners.delete(listener);
		},
		onGpuLost: (listener) => {
			gpuLost.add(listener);
			return () => gpuLost.delete(listener);
		},
		commands: () =>
			ran().map((command): MarkedCommand => ({ id: command.id, ...markOf(command) })),
		output: (id) => {
			const command = ran().find((held) => held.id === id);
			if (!command?.start) return null;
			const start = placeOf(command.start);
			// It ends where its end mark sits, or at the cursor while it runs,
			// and a line it ended on with nothing printed isn't its.
			const end = command.end ? placeOf(command.end) : { y: lastRow(), x: buffer().cursorX };
			const to: Place = end.x ? end : { y: end.y - 1 };
			const first = numberOf(start.y);
			const mark = markOf(command);
			if (to.y < start.y) return { text: "", lines: { first, last: first }, mark };
			const text = textBetween(start, to).replace(/\s+$/, "");
			return { text, lines: { first, last: numberOf(to.y) }, mark };
		},
		jump: (back) => {
			const commands = ran();
			const top = buffer().viewportY;
			const bottom = buffer().baseY;
			// From the command it showed last while the screen still shows it,
			// from the end when back at the end, and from the top otherwise.
			const from =
				shown && commands.includes(shown) && top === Math.min(lineOf(shown), bottom)
					? lineOf(shown)
					: back && top === bottom
						? Infinity
						: top;
			const next = back
				? commands.findLast((command) => lineOf(command) < from)
				: commands.find((command) => lineOf(command) > from);
			const was = shown;
			shown = next ?? null;
			if (was) paint(was);
			if (next) {
				paint(next);
				terminal.scrollToLine(lineOf(next));
			} else if (!back) terminal.scrollToBottom();
			return next !== undefined;
		},
		onMark: (listener) => {
			markListeners.add(listener);
			return () => markListeners.delete(listener);
		},
		dispose: () => {
			stopPalette();
			dropGpu();
			terminal.dispose();
			element.remove();
		},
	};
	return view;
};
