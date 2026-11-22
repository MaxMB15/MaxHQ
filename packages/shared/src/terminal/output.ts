// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A command's output on its way to an agent. Its pill follows the command
// while it runs, and when the message goes it takes the command's last
// lines: what it wrote in all once it ended, or what the live terminal
// shows so far.

import type { TerminalId, TerminalStatus, TimelineItem, Timestamp } from "../bindings";
import type { OutputRef } from "../pills/draft";
import type { TimelineState } from "../timeline/apply";
import { stripAnsi } from "../timeline/ansi";

/** A command you ran with `!`, as the timeline holds it. */
export type UserRunItem = Extract<TimelineItem, { kind: "user_run" }>;

/** How many of a command's last lines go to the agent. */
export const OUTPUT_LINES = 200;

/** A text's last `count` lines, and how many it has in all. A last line break makes no line. */
export const lastLines = (text: string, count: number): { text: string; total: number } => {
	const body = text.replace(/\r?\n$/, "");
	if (body === "") return { text: "", total: 0 };
	const lines = body.split("\n");
	return { text: lines.slice(-count).join("\n"), total: lines.length };
};

/** What a command that just started needs to name its pill. */
export type StartedRun = {
	readonly command: string;
	readonly deviceName: string;
	readonly cwd: string;
	readonly terminal: TerminalId;
	readonly at: Timestamp;
};

/** A command that just started, as the pill that waits for its output in the composer. */
export const startedOutput = (run: StartedRun): OutputRef => ({
	type: "output",
	command: run.command,
	device_name: run.deviceName,
	cwd: run.cwd,
	exit_code: null,
	running: true,
	ran_at: run.at,
	duration_ms: null,
	lines_total: 0,
	text: "",
	terminal: run.terminal,
	lines: null,
});

/** The item for the command in this terminal, if the timeline holds it. */
export const userRunOf = (
	state: Pick<TimelineState, "items" | "order">,
	terminal: TerminalId
): UserRunItem | undefined => {
	for (let index = state.order.length - 1; index >= 0; index -= 1) {
		const item = state.items.get(state.order[index]!);
		if (item?.kind === "user_run" && item.terminal === terminal) return item;
	}
	return undefined;
};

/**
 * How a command stands: its terminal's status while the view holds the
 * terminal, which changes first, and its item's after that. Null when
 * neither is here.
 */
export const runStatus = (
	terminal: { readonly status: TerminalStatus } | undefined,
	item: Pick<UserRunItem, "process"> | undefined
): TerminalStatus | null => terminal?.status ?? item?.process ?? null;

/**
 * The pill's state as a status says it, with the signal that ended the
 * command and whether your Stop did.
 */
export const withStatus = (ref: OutputRef, status: TerminalStatus | null): OutputRef =>
	status === null
		? ref
		: status.state === "running"
			? { ...ref, running: true, exit_code: null, signal: null, stopped: false }
			: {
					...ref,
					running: false,
					exit_code: status.code,
					signal: status.signal,
					stopped: status.stopped === true,
				};

/** What the composer has of a command when the message goes. */
export type OutputSources = {
	/** The command's item in the timeline. */
	readonly item?: UserRunItem;
	/** Its terminal, while the view holds it. */
	readonly terminal?: { readonly status: TerminalStatus };
	/** The live terminal's last lines, while this window holds it. */
	readonly live?: { readonly text: string; readonly total: number } | null;
	readonly now: Timestamp;
};

/**
 * The pill as the message takes it: its last 200 lines, from the whole
 * output once the command ended and from the live terminal while it runs,
 * with how it stands. Lines picked in the drawer go as they were picked.
 */
export const fillOutput = (ref: OutputRef, sources: OutputSources): OutputRef => {
	if (ref.lines) return ref;
	const { item, live, now } = sources;
	const shown = withStatus(ref, runStatus(sources.terminal, item));
	if (!shown.running && item) {
		const { text } = lastLines(stripAnsi(item.output), OUTPUT_LINES);
		return { ...shown, text, lines_total: item.lines, duration_ms: item.duration_ms };
	}
	const sofar = shown.running ? Math.max(0, now - shown.ran_at) : shown.duration_ms;
	if (live) return { ...shown, text: live.text, lines_total: live.total, duration_ms: sofar };
	if (item) {
		const { text } = lastLines(stripAnsi(item.output), OUTPUT_LINES);
		return { ...shown, text, lines_total: item.lines, duration_ms: sofar };
	}
	return { ...shown, duration_ms: sofar };
};
