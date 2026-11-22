// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where a command you ran with `!` goes when it wants more than a card can
// give. A program that takes the whole screen, such as vim from `git
// commit`, opens the terminal panel by itself, but only for the newest
// command this window started, in the last few seconds, and only while
// focus is free to move there. Its card then says the program is in the
// terminal. A picker that reads keys one at a time can take your keys in
// the card instead, when the device answers the terminal's queries itself.
// The phone says the same in its own words, with no controls of its own.

import type { TerminalId, Timestamp } from "../bindings";
import { t } from "../i18n";

import type { TerminalEntity } from "./select";

/** How long after you ran a command its program may still open the panel by itself. */
export const JUST_RAN_MS = 10_000;

/** The newest command this window started: a block of the session, or a fresh run. */
export type JustRan = {
	readonly terminal: TerminalId;
	/** The session block, or null for a command in a shell of its own. */
	readonly block: number | null;
	readonly at: Timestamp;
};

/** What of a terminal says how its program wants your keys. */
type Modes = Pick<
	TerminalEntity,
	"full_screen" | "reads_keys" | "waiting" | "answers" | "password_prompt"
>;

/** Whether a running command's program shows the whole screen, so it's in the terminal. */
export const inTerminal = (running: boolean, terminal: Modes | undefined): boolean =>
	running && terminal?.full_screen === true;

/**
 * Whether a card can hand your keys to the program: it reads keys one at a
 * time and isn't full screen, and the device answers the terminal's queries,
 * so the card's view never has to. With a device before protocol 59 the
 * card stays view-only.
 */
export const takesKeys = (running: boolean, terminal: Modes | undefined): boolean =>
	running &&
	terminal?.answers === true &&
	terminal.reads_keys === true &&
	terminal.full_screen !== true &&
	terminal.password_prompt !== true;

/** Whether a program that reads keys waits for one, so the card says "Waiting for keys". */
export const waitsForKeys = (running: boolean, terminal: Modes | undefined): boolean =>
	running &&
	terminal?.reads_keys === true &&
	terminal.waiting === true &&
	terminal.full_screen !== true &&
	terminal.password_prompt !== true;

/**
 * Whether a command that just turned full screen opens the panel by itself:
 * it's the newest one this window started, `ran`, at most `JUST_RAN_MS` ago,
 * and focus is free to move. Anything else, such as an agent's command, one
 * started on another device or a program that goes full screen later, only
 * shows the card's note.
 */
export const opensPanel = (
	newest: JustRan | null,
	ran: { readonly terminal: TerminalId; readonly block: number | null },
	now: Timestamp,
	focusFree: boolean
): boolean =>
	newest !== null &&
	focusFree &&
	newest.terminal === ran.terminal &&
	newest.block === ran.block &&
	now - newest.at >= 0 &&
	now - newest.at <= JUST_RAN_MS;

/** How a running command's program wants you, as a card or row says it. */
export type Handoff = "in_terminal" | "keys";

/** The hand-off a running command shows, or null when it needs none. */
export const handoffOf = (running: boolean, terminal: Modes | undefined): Handoff | null =>
	inTerminal(running, terminal) ? "in_terminal" : waitsForKeys(running, terminal) ? "keys" : null;

/** The chip's words: "In the terminal" or "Waiting for keys". */
export const handoffLabel = (handoff: Handoff): string =>
	handoff === "in_terminal" ? t("In the terminal") : t("Waiting for keys");

/** The phone's words, which send you to the Mac since it has no keys to give. */
export const handoffPhoneText = (handoff: Handoff): string =>
	handoff === "in_terminal"
		? t("In the terminal · Open on the Mac")
		: t("Waiting for keys · Open on the Mac");
