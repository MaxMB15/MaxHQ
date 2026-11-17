// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the drawer reads from a terminal: the command it runs now, which a
// tab's dot and the questions before closing a tab or deleting its agent
// name, and the Output pill that text picked in it makes.
//
// A command you ran with `!` runs until it exits. A shell sets its title to
// the command it runs, and to its own name or its folder while it waits at
// the prompt, so a title that's neither names the command. `maxd` doesn't
// say yet what runs in the foreground, so a tab you renamed reads as running
// unless the caller knows the name is yours.

import type { AgentId, LineSpan } from "../bindings";
import type { OutputRef } from "../pills/draft";
import { all, type ViewState } from "../view/replica";

import type { TerminalEntity } from "./select";

const SHELLS: ReadonlySet<string> = new Set([
	"zsh",
	"bash",
	"fish",
	"sh",
	"dash",
	"ksh",
	"tcsh",
	"nu",
	"pwsh",
	"login",
]);

/** Whether a shell's title is the one it sets at the prompt. */
const waitingTitle = (title: string): boolean => {
	const word = (title.split(/\s+/)[0] ?? "").replace(/^-/, "");
	return (
		SHELLS.has(word.slice(word.lastIndexOf("/") + 1)) ||
		// A folder, such as `~/code/example-app`, or `you@host: ~/code`.
		/^[~/]/.test(title) ||
		/^[^\s@]+@[^\s:]+:/.test(title)
	);
};

/**
 * The command the terminal runs now, or null when it's done or a shell
 * waits at its prompt. `named` is what you renamed the tab to, which says
 * nothing of what runs.
 */
export const runningCommand = (terminal: TerminalEntity, named?: string): string | null => {
	if (terminal.status.state !== "running") return null;
	if (terminal.kind.type === "run") return terminal.kind.command;
	const title = terminal.title.trim();
	if (title === "" || title === named || waitingTitle(title)) return null;
	return title;
};

/**
 * The commands still running in these agents' terminals and their
 * sub-agents', each once, which a delete would stop. `named` maps a
 * terminal to what you renamed its tab to.
 */
export const runningCommands = (
	view: ViewState,
	agents: ReadonlySet<AgentId>,
	named: Readonly<Record<string, string>> = {}
): string[] => {
	const goes = (agent: AgentId) =>
		agents.has(agent) || agents.has(view.tables.agent.get(agent)?.parent ?? "");
	const commands = all(view, "terminal")
		.filter((terminal) => goes(terminal.agent))
		.map((terminal) => runningCommand(terminal, named[terminal.id]));
	return [...new Set(commands.filter((command) => command !== null))];
};

/** What a tab shows: its title, or the command it was opened for. */
export const tabTitle = (terminal: TerminalEntity): string =>
	terminal.title.trim() ||
	(terminal.kind.type === "run" ? terminal.kind.command : terminal.cwd.split("/").pop() || "~");

/**
 * The Output pill for text picked in a terminal: the lines it sits on in
 * all the terminal printed, and whether the terminal still runs or how it
 * exited. `total` is how many lines it printed in all.
 */
export const pickedOutput = (
	terminal: TerminalEntity,
	deviceName: string,
	picked: { readonly text: string; readonly lines: LineSpan },
	total: number
): OutputRef => ({
	type: "output",
	command: runningCommand(terminal) ?? tabTitle(terminal),
	device_name: deviceName,
	cwd: terminal.cwd,
	exit_code: terminal.status.state === "exited" ? terminal.status.code : null,
	running: terminal.status.state === "running",
	ran_at: terminal.started_at,
	duration_ms: null,
	lines_total: Math.max(total, picked.lines.last),
	text: picked.text,
	terminal: terminal.id,
	lines: picked.lines,
});
