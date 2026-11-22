// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	handoffLabel,
	handoffOf,
	handoffPhoneText,
	inTerminal,
	JUST_RAN_MS,
	type JustRan,
	opensPanel,
	takesKeys,
	waitsForKeys,
} from "./handoff";

/** A terminal's modes, as a device of protocol 59 or later sends them. */
const modes = (
	over: Partial<{
		full_screen: boolean;
		reads_keys: boolean;
		waiting: boolean;
		answers: boolean;
		password_prompt: boolean;
	}> = {}
) => ({
	full_screen: false,
	reads_keys: false,
	waiting: false,
	answers: true,
	password_prompt: false,
	...over,
});

/** What a device before protocol 59 sends: none of the modes. */
const older = { password_prompt: false };

describe("a program in the terminal", () => {
	it("is in the terminal while it runs full screen", () => {
		expect(inTerminal(true, modes({ full_screen: true }))).toBe(true);
		expect(inTerminal(false, modes({ full_screen: true }))).toBe(false);
		expect(inTerminal(true, modes())).toBe(false);
		expect(inTerminal(true, undefined)).toBe(false);
		expect(inTerminal(true, older)).toBe(false);
	});

	it("takes keys in the card only when the device answers for the terminal", () => {
		expect(takesKeys(true, modes({ reads_keys: true }))).toBe(true);
		expect(takesKeys(true, modes({ reads_keys: true, answers: false }))).toBe(false);
		expect(takesKeys(true, modes({ reads_keys: true, full_screen: true }))).toBe(false);
		expect(takesKeys(true, modes({ reads_keys: true, password_prompt: true }))).toBe(false);
		expect(takesKeys(false, modes({ reads_keys: true }))).toBe(false);
		expect(takesKeys(true, modes())).toBe(false);
		expect(takesKeys(true, older)).toBe(false);
	});

	it("waits for keys while a program that reads them waits", () => {
		expect(waitsForKeys(true, modes({ reads_keys: true, waiting: true }))).toBe(true);
		expect(waitsForKeys(true, modes({ reads_keys: true }))).toBe(false);
		expect(waitsForKeys(true, modes({ waiting: true }))).toBe(false);
		expect(
			waitsForKeys(true, modes({ reads_keys: true, waiting: true, full_screen: true }))
		).toBe(false);
		expect(
			waitsForKeys(true, modes({ reads_keys: true, waiting: true, password_prompt: true }))
		).toBe(false);
		// The phone says it even with a device that doesn't answer for the terminal.
		expect(waitsForKeys(true, modes({ reads_keys: true, waiting: true, answers: false }))).toBe(
			true
		);
	});

	it("names the hand-off, full screen first", () => {
		expect(handoffOf(true, modes({ full_screen: true, reads_keys: true, waiting: true }))).toBe(
			"in_terminal"
		);
		expect(handoffOf(true, modes({ reads_keys: true, waiting: true }))).toBe("keys");
		expect(handoffOf(true, modes({ reads_keys: true }))).toBeNull();
		expect(handoffOf(false, modes({ full_screen: true }))).toBeNull();
		expect(handoffOf(true, older)).toBeNull();
	});

	it("says it in words for the card and for the phone", () => {
		expect(handoffLabel("in_terminal")).toBe("In the terminal");
		expect(handoffLabel("keys")).toBe("Waiting for keys");
		expect(handoffPhoneText("in_terminal")).toBe("In the terminal · Open on the Mac");
		expect(handoffPhoneText("keys")).toBe("Waiting for keys · Open on the Mac");
	});
});

describe("opening the panel by itself", () => {
	const AT = 1_000_000;
	const newest: JustRan = { terminal: "session-1", block: 4, at: AT };

	it("opens for the newest command, a moment after it started, with focus free", () => {
		expect(opensPanel(newest, { terminal: "session-1", block: 4 }, AT + 1_500, true)).toBe(
			true
		);
		expect(
			opensPanel(newest, { terminal: "session-1", block: 4 }, AT + JUST_RAN_MS, true)
		).toBe(true);
	});

	it("stays shut once the start is more than ten seconds old", () => {
		expect(JUST_RAN_MS).toBe(10_000);
		expect(
			opensPanel(newest, { terminal: "session-1", block: 4 }, AT + JUST_RAN_MS + 1, true)
		).toBe(false);
		expect(opensPanel(newest, { terminal: "session-1", block: 4 }, AT - 1, true)).toBe(false);
	});

	it("stays shut for another block, another terminal or nothing started here", () => {
		expect(opensPanel(newest, { terminal: "session-1", block: 3 }, AT + 100, true)).toBe(false);
		expect(opensPanel(newest, { terminal: "session-1", block: null }, AT + 100, true)).toBe(
			false
		);
		expect(opensPanel(newest, { terminal: "session-2", block: 4 }, AT + 100, true)).toBe(false);
		expect(opensPanel(null, { terminal: "session-1", block: 4 }, AT + 100, true)).toBe(false);
	});

	it("stays shut while focus is busy elsewhere", () => {
		expect(opensPanel(newest, { terminal: "session-1", block: 4 }, AT + 100, false)).toBe(
			false
		);
	});

	it("opens for a fresh run, which has no block", () => {
		const run: JustRan = { terminal: "run-7", block: null, at: AT };
		expect(opensPanel(run, { terminal: "run-7", block: null }, AT + 100, true)).toBe(true);
	});
});
