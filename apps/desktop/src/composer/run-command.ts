// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Running a command you typed after `!`, from the composer or a card's Run
// again. The card it makes takes the timeline's width, so the terminal
// opens that many columns wide. Unless it runs privately, its output pill
// goes in the composer at once and follows the command from there.

import { t } from "@maxhq/shared/i18n";

import type { AgentId, TerminalId } from "@maxhq/shared/bindings";
import { attempt, failureOf } from "@maxhq/shared/composer/send";
import type { DataSource } from "@maxhq/shared/data/source";
import { runCommand, openedTerminal, type TerminalSize } from "@maxhq/shared/terminal/requests";
import { startedOutput } from "@maxhq/shared/terminal/output";
import { terminalById } from "@maxhq/shared/terminal/select";
import { runPlace } from "@maxhq/shared/terminal/shell";
import type { ViewState } from "@maxhq/shared/view/replica";

import { quoteInbox } from "./quote-inbox";

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

/** What starting a command came to: its terminal, or why it didn't start. */
export type Started =
	| { readonly terminal: TerminalId; readonly error?: undefined }
	| { readonly error: string; readonly terminal?: undefined };

/**
 * Runs `command` for the agent. Unless it runs privately, its output pill
 * goes to the agent's composer, which takes it now or when it opens.
 */
export const startRun = async (
	source: Pick<DataSource, "openTerminal">,
	view: () => ViewState,
	run: {
		readonly agent: AgentId;
		readonly command: string;
		readonly private: boolean;
		readonly size: TerminalSize;
	}
): Promise<Started> => {
	const result = await attempt(() =>
		runCommand(source, run.agent, run.command, { private: run.private, size: run.size })
	);
	const terminal = openedTerminal(result);
	if (terminal === null) {
		const reason = failureOf(result);
		return {
			error: reason
				? t("Couldn't run the command. {{reason}}.", { reason })
				: t("Couldn't run the command."),
		};
	}
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
	return { terminal };
};
