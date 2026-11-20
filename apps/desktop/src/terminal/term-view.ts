// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What draws one terminal. The registry and the drawer use only this, and
// `term-view-xterm.ts` is the one file that knows the renderer, so it can
// change behind it and tests can stand one in.

import type { LineSpan } from "@maxhq/shared/bindings";
import type { CommandMark } from "@maxhq/shared/terminal/marks";

/** Text you picked in a terminal, and where it sits in all it printed. */
export type Picked = { readonly text: string; readonly lines: LineSpan };

/** A command its shell marked, while the view holds it. */
export type MarkedCommand = CommandMark & {
	/** Names it in the view, counting up from 1. */
	readonly id: number;
	/**
	 * Where its command mark starts in all the terminal printed, as the
	 * device counts it, or null when the writes didn't say.
	 */
	readonly offset: number | null;
};

/** What a marked command printed, and the lines that takes, with what its marks say. */
export type CommandOutput = Picked & { readonly mark: CommandMark };

/** One terminal's screen and scrollback. */
export type TermView = {
	/** The element the terminal draws in, which moves between hosts. */
	readonly element: HTMLElement;
	/**
	 * Draws what the terminal printed, with where those bytes start in all
	 * it printed when that's known. The screen takes them in a moment
	 * later, and then `done` runs.
	 */
	readonly write: (bytes: Uint8Array, offset?: number, done?: () => void) => void;
	/** Forgets the screen and scrollback, for output that starts over. */
	readonly clear: () => void;
	/** Sets the size in cells. */
	readonly resize: (cols: number, rows: number) => void;
	/**
	 * Sizes the terminal to fill its element and answers with the size, or
	 * null while the element has no size, as when it's hidden.
	 */
	readonly fit: () => { readonly cols: number; readonly rows: number } | null;
	/** The size in cells. */
	readonly size: () => { readonly cols: number; readonly rows: number };
	/** How many rows the output takes so far, up to the cursor. */
	readonly usedRows: () => number;
	readonly focus: () => void;
	/** Whether keys go to it: an interactive one shows a cursor and takes keys. */
	readonly setInteractive: (interactive: boolean) => void;
	/** The picked text, or null when there's none. */
	readonly selection: () => Picked | null;
	readonly clearSelection: () => void;
	/**
	 * The last `lines` lines as plain text, and how many it printed in all,
	 * counted the way `selection` counts.
	 */
	readonly text: (lines: number) => { readonly text: string; readonly total: number };
	/** Types `text` as a paste, which a shell that asks for it gets bracketed. */
	readonly paste: (text: string) => void;
	/** Finds `query` from the selection on, backwards with `back`. False when it isn't there. */
	readonly find: (query: string, back: boolean) => boolean;
	/** Draws with the GPU or without. A view that lost its GPU context stays without. */
	readonly setGpu: (on: boolean) => void;
	/** Calls back with what you type, as the terminal encodes it. */
	readonly onData: (listener: (data: string) => void) => () => void;
	/** Calls back when the shell sets the title. */
	readonly onTitle: (listener: (title: string) => void) => () => void;
	/** Calls back when the picked text changes. */
	readonly onSelection: (listener: () => void) => () => void;
	/** Calls back when it takes or loses focus. */
	readonly onFocus: (listener: (focused: boolean) => void) => () => void;
	/** Calls back once the GPU drops its context, after the view went back to drawing without. */
	readonly onGpuLost: (listener: () => void) => () => void;
	/**
	 * Whether it takes the OSC 133 in its output as its shell's marks, as it
	 * may once the device says its holder lets only those through. Without,
	 * it takes none. Either way it drops them, so they draw nothing.
	 */
	readonly setMarks: (on: boolean) => void;
	/**
	 * The commands its shell marked that the view still holds, oldest first.
	 * A shell without marks has none.
	 */
	readonly commands: () => readonly MarkedCommand[];
	/** What a marked command printed so far, or null once the view let it go. */
	readonly output: (id: number) => CommandOutput | null;
	/**
	 * Scrolls to the command before the one it showed last, or after it with
	 * `back` false, and to the end past the last. False when there's none.
	 */
	readonly jump: (back: boolean) => boolean;
	/** Calls back when you click a command's mark, with the mark to place a menu by. */
	readonly onMark: (listener: (id: number, anchor: Element) => void) => () => void;
	readonly dispose: () => void;
};

/** Makes the view for one terminal. */
export type MakeView = () => TermView;

/**
 * What the command whose mark starts at `start` printed so far, and how
 * many lines that is, or null when the view holds no such command. A shell
 * started inside it marks its own commands after it, so the last command
 * the view holds may be another.
 */
export const commandText = (
	view: Pick<TermView, "commands" | "output">,
	start: number
): { readonly text: string; readonly total: number } | null => {
	const command = view.commands().find((each) => each.offset === start);
	const shown = command ? view.output(command.id) : null;
	if (!shown) return null;
	return { text: shown.text, total: shown.text === "" ? 0 : shown.text.split("\n").length };
};
