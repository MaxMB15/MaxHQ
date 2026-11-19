// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the drawer reads from a terminal: whether a command runs in it and
// the name it goes by, which a tab's dot and the questions before closing a
// tab or deleting its agent show, and the Output pill that text picked in it
// makes.
//
// A command you ran with `!` runs until it exits: a shell may run it in its
// own place, so nothing else ever takes the foreground. A shell runs one
// while `maxd` says it's busy. Its title says nothing about that, since a
// shell or its theme may set any title at the prompt, and you may rename it.

import type { AgentId, LineSpan } from "../bindings";
import type { OutputRef } from "../pills/draft";
import { all, type ViewState } from "../view/replica";

import type { TerminalEntity } from "./select";

/** What a tab shows: its title, or the command it was opened for. */
export const tabTitle = (terminal: TerminalEntity): string =>
	terminal.title.trim() ||
	(terminal.kind.type === "run" ? terminal.kind.command : terminal.cwd.split("/").pop() || "~");

/** Whether a command runs in the terminal now. */
export const runsCommand = (terminal: Pick<TerminalEntity, "kind" | "status" | "busy">): boolean =>
	terminal.status.state === "running" && (terminal.kind.type === "run" || terminal.busy === true);

/**
 * The name of the command the terminal runs now, or null when it's done or a
 * shell waits at its prompt: the command you ran, or a shell's tab title.
 */
export const runningCommand = (terminal: TerminalEntity): string | null => {
	if (!runsCommand(terminal)) return null;
	return terminal.kind.type === "run" ? terminal.kind.command : tabTitle(terminal);
};

/**
 * The commands still running in these agents' terminals and their
 * sub-agents', each once, which a delete would stop.
 */
export const runningCommands = (view: ViewState, agents: ReadonlySet<AgentId>): string[] => {
	const goes = (agent: AgentId) =>
		agents.has(agent) || agents.has(view.tables.agent.get(agent)?.parent ?? "");
	const commands = all(view, "terminal")
		.filter((terminal) => goes(terminal.agent))
		.map(runningCommand);
	return [...new Set(commands.filter((command) => command !== null))];
};

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
