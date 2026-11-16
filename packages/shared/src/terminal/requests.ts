// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The terminal requests, as calls of a data source: open a shell or run a
// command, attach to its output and let go of it, resize it, and interrupt,
// close, rename or move it. Attaching, detaching and resizing act on this
// app's link alone, so they answer once the request went out. The others
// answer with what the device made of them.

import type { AgentId, TerminalId, TerminalKind } from "../bindings";
import type { Ask } from "../data/request-source";
import type { ActionResult, CommandResult } from "../data/source";

/** A terminal's size in cells. */
export type TerminalSize = { readonly cols: number; readonly rows: number };

/** The terminal calls of a data source. */
export type TerminalCalls = {
	/**
	 * Opens a terminal in the agent's folder, on its device: a shell for its
	 * drawer, or one command you ran with `!`. Answers `terminal_opened` with
	 * the new terminal.
	 */
	readonly openTerminal: (
		agent: AgentId,
		kind: TerminalKind,
		size: TerminalSize
	) => Promise<ActionResult>;
	/**
	 * Asks the device to send this app the terminal's output, from just past
	 * byte `after`, or from all it still holds when null.
	 */
	readonly attachTerminal: (terminal: TerminalId, after: number | null) => Promise<CommandResult>;
	/** Asks the device to stop sending this app the terminal's output. */
	readonly detachTerminal: (terminal: TerminalId) => Promise<CommandResult>;
	/** Sets the terminal's size. The client you last typed in owns it. */
	readonly resizeTerminal: (terminal: TerminalId, size: TerminalSize) => Promise<CommandResult>;
	/** Sends the terminal Ctrl-C. */
	readonly interruptTerminal: (terminal: TerminalId) => Promise<ActionResult>;
	/** Hangs up what runs in the terminal, or kills it with `force`, then forgets it. */
	readonly closeTerminal: (terminal: TerminalId, force?: boolean) => Promise<ActionResult>;
	/** Renames the terminal's tab. Empty goes back to the name the shell sets. */
	readonly renameTerminal: (terminal: TerminalId, title: string) => Promise<ActionResult>;
	/** Moves a command you ran with `!` to the agent's drawer, where it goes on as a tab. */
	readonly moveTerminal: (terminal: TerminalId) => Promise<ActionResult>;
};

/** The terminal calls, each one request over `ask`. */
export const terminalCalls = (ask: Ask): TerminalCalls => ({
	openTerminal: (agent, runs, { cols, rows }) =>
		ask({ kind: "terminal_open", agent, runs, cols, rows }),
	attachTerminal: (terminal, after) => ask({ kind: "terminal_attach", terminal, after }),
	detachTerminal: (terminal) => ask({ kind: "terminal_detach", terminal }),
	resizeTerminal: (terminal, { cols, rows }) =>
		ask({ kind: "terminal_resize", terminal, cols, rows }),
	interruptTerminal: (terminal) => ask({ kind: "terminal_interrupt", terminal }),
	closeTerminal: (terminal, force = false) => ask({ kind: "terminal_close", terminal, force }),
	renameTerminal: (terminal, title) => ask({ kind: "terminal_rename", terminal, title }),
	moveTerminal: (terminal) => ask({ kind: "terminal_move", terminal }),
});

/** Opens a shell in the agent's drawer. */
export const openShell = (
	calls: Pick<TerminalCalls, "openTerminal">,
	agent: AgentId,
	size: TerminalSize
): Promise<ActionResult> => calls.openTerminal(agent, { type: "shell" }, size);

/**
 * Runs one command for the agent, as typed after `!`. A private one, typed
 * after `!!`, reaches the agent only when you attach its output to a message.
 */
export const runCommand = (
	calls: Pick<TerminalCalls, "openTerminal">,
	agent: AgentId,
	command: string,
	options: { readonly private: boolean; readonly size: TerminalSize }
): Promise<ActionResult> =>
	calls.openTerminal(agent, { type: "run", command, private: options.private }, options.size);

/** The terminal an open answered with, or null when it didn't open one. */
export const openedTerminal = (result: ActionResult): TerminalId | null =>
	result.status === "ok" && result.data.status === "terminal_opened"
		? result.data.terminal
		: null;
