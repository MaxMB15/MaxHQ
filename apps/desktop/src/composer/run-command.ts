// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Running a command you typed after `!`, from the composer or a card's Run
// again. The card it makes takes the timeline's width, so the terminal
// opens that many columns wide. Unless it runs privately, its output pill
// goes in the composer at once and follows the command from there. On a
// device with sessions it runs in the agent's session instead, whose pill
// stands for all its commands.

import { t } from "@maxhq/shared/i18n";

import type { AgentId, TerminalId } from "@maxhq/shared/bindings";
import { attempt, failureOf } from "@maxhq/shared/composer/send";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { runCommand, openedTerminal, type TerminalSize } from "@maxhq/shared/terminal/requests";
import { startedOutput } from "@maxhq/shared/terminal/output";
import { type SessionRan, sessionRanOf } from "@maxhq/shared/terminal/session";
import { terminalById } from "@maxhq/shared/terminal/select";
import { runPlace } from "@maxhq/shared/terminal/shell";
import type { ViewState } from "@maxhq/shared/view/replica";

import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";

/** How many rows a command's terminal has, as many as its card shows. */
export const RUN_ROWS = 24;

/** The fewest columns a terminal opens with, however narrow the window. */
const MIN_COLS = 20;

/** What a card's output takes of its width: its padding and borders. */
const CARD_EDGES = 26;

/** The size a command's terminal opens with, for a card `width` pixels wide. */
export const runSize = (width: number, cellWidth: number): TerminalSize => ({
	cols: Math.max(MIN_COLS, Math.floor((width - CARD_EDGES) / Math.max(1, cellWidth))),
	rows: RUN_ROWS,
});

/**
 * The commands this window started that haven't asked for a password yet.
 * The first time one does, its secure field takes focus, so a password
 * typed straight after the command goes to it and not to the composer.
 */
const startedHere = new Set<TerminalId>();

/** Whether this window started the command in `terminal` and it hasn't asked for a password yet. */
export const firstAskHere = (terminal: TerminalId): boolean => startedHere.has(terminal);

/** The command in `terminal` asked for a password, so later asks don't take focus. */
export const askedHere = (terminal: TerminalId): void => {
	startedHere.delete(terminal);
};

/** What starting a command came to: its terminal, or why it didn't start. */
export type Started =
	| { readonly terminal: TerminalId; readonly error?: undefined }
	| { readonly error: string; readonly terminal?: undefined };

/** A command to run for an agent. */
type Run = {
	readonly agent: AgentId;
	readonly command: string;
	readonly private: boolean;
	readonly size: TerminalSize;
};

/** Why a command didn't run. */
const notRun = (result: ActionResult): string => {
	const reason = failureOf(result);
	return reason
		? t("Couldn't run the command. {{reason}}.", { reason })
		: t("Couldn't run the command.");
};

/**
 * Runs `command` for the agent. Unless it runs privately, its output pill
 * goes to the agent's composer, which takes it now or when it opens.
 */
export const startRun = async (
	source: Pick<DataSource, "openTerminal">,
	view: () => ViewState,
	run: Run
): Promise<Started> => {
	const result = await attempt(() =>
		runCommand(source, run.agent, run.command, { private: run.private, size: run.size })
	);
	const terminal = openedTerminal(result);
	if (terminal === null) return { error: notRun(result) };
	follow(view, run, terminal);
	return { terminal };
};

/** What running a command in the session came to: its block, a fresh shell, or why it didn't run. */
export type SessionStarted = Started | { readonly ran: SessionRan; readonly error?: undefined };

/**
 * Runs `command` in the agent's session, or in a fresh shell with `fresh`
 * or when the session can't take it. A block's pill is the session's,
 * which the composer keeps; a fresh shell's output pill goes in as
 * `startRun`'s does.
 */
export const startSessionRun = async (
	source: Pick<DataSource, "sessionRun">,
	view: () => ViewState,
	run: Run & { readonly fresh: boolean }
): Promise<SessionStarted> => {
	const result = await attempt(() =>
		source.sessionRun(run.agent, run.command, {
			private: run.private,
			fresh: run.fresh,
			size: run.size,
		})
	);
	const ran = sessionRanOf(result);
	if (ran) {
		// Each command's first ask for a password takes focus.
		startedHere.add(ran.terminal);
		return { ran };
	}
	const terminal = openedTerminal(result);
	if (terminal === null) return { error: notRun(result) };
	follow(view, run, terminal);
	return { terminal };
};

/** Notes that this window started the command, and puts its output pill in unless it's private. */
const follow = (view: () => ViewState, run: Run, terminal: TerminalId) => {
	startedHere.add(terminal);
	if (!run.private) {
		const state = view();
		const place = runPlace(state, run.agent);
		quoteInbox.send(
			run.agent,
			startedOutput({
				command: run.command,
				deviceName: place?.deviceName ?? "",
				cwd: terminalById(state, terminal)?.cwd ?? place?.folder ?? "",
				terminal,
				at: Date.now(),
			}),
			false
		);
	}
};
