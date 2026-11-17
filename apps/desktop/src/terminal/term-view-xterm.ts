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

import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import { onMac } from "@/native/platform";

import { readPalette, watchPalette } from "./palette";
import type { MakeView, Picked, TermView } from "./term-view";

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
			const lines: string[] = [];
			// The rows below that continue the line being read.
			let tail = "";
			for (let y = lastRow(); y >= 0 && lines.length < count; y--) {
				const line = buffer().getLine(y);
				// A row a wrap continues keeps its spaces, which the wrap may need.
				tail = (line?.translateToString(tail === "") ?? "") + tail;
				if (line?.isWrapped) continue;
				lines.unshift(tail);
				tail = "";
			}
			const total = firstNumber() + linesThrough(lastRow()) - 1;
			return { text: lines.join("\n").replace(/\s+$/, ""), total };
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
		dispose: () => {
			stopPalette();
			dropGpu();
			terminal.dispose();
			element.remove();
		},
	};
	return view;
};
