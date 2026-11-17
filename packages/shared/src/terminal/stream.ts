// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A terminal's bytes, both ways. Output comes with its offset in all the
// terminal printed, so a view that attaches again asks from where it ended
// and skips what it already shows. The desktop gets it through a raw Tauri
// channel and the sample data makes it up; the phone will get it through
// its own core. Nothing here keeps what you type.

import type { TerminalId } from "../bindings";
import type { TerminalSize } from "./requests";

/** What reaches a view of a terminal. */
export type TerminalEvent =
	/** Bytes the terminal printed, starting at byte `offset` of all it printed. */
	| { readonly type: "output"; readonly offset: number; readonly bytes: Uint8Array }
	/**
	 * The device no longer holds what came before `from`, so the next bytes
	 * start there.
	 */
	| { readonly type: "reset"; readonly from: number };

/** A terminal's bytes between a client and the device that holds it. */
export type TerminalStream = {
	/**
	 * Starts the output after byte `after`, or from the oldest the device
	 * still holds when null. Attaching again starts over from `after`.
	 * Rejects with a sentence to show when it can't.
	 */
	readonly attach: (terminal: TerminalId, after: number | null) => Promise<void>;
	/** Stops the output. */
	readonly detach: (terminal: TerminalId) => void;
	/** Tells the terminal its size in cells. */
	readonly resize: (terminal: TerminalId, size: TerminalSize) => void;
	/** Sends what you type. It isn't kept or logged anywhere on the way. */
	readonly input: (terminal: TerminalId, bytes: Uint8Array) => void;
	/** Calls `listener` with the terminal's output and resets, until the returned call. */
	readonly listen: (terminal: TerminalId, listener: (event: TerminalEvent) => void) => () => void;
};

/**
 * The offset of the last byte a view that ends at `end` shows, which an
 * attach names to go on after it. Null when it shows none yet.
 */
export const lastByte = (end: number | null): number | null =>
	end === null || end === 0 ? null : end - 1;

/** What a view does with an event: clear first or not, then write `bytes`. */
export type Step = {
	readonly clear: boolean;
	readonly bytes: Uint8Array;
	/** Where the view ends after it. */
	readonly end: number;
};

const NOTHING = new Uint8Array(0);

/**
 * What a view that ends at `end`, or shows nothing yet when null, does with
 * an event. Bytes it already shows are skipped, which a second window's
 * attach or a reconnect replays. A gap means the device dropped what came
 * between, so the view clears and goes on from there. Null when there's
 * nothing to do.
 */
export const stepFor = (end: number | null, event: TerminalEvent): Step | null => {
	if (event.type === "reset") {
		if (end !== null && event.from <= end) return null;
		return { clear: end !== null, bytes: NOTHING, end: event.from };
	}
	const { offset, bytes } = event;
	const after = offset + bytes.length;
	if (end === null) return { clear: false, bytes, end: after };
	if (offset > end) return { clear: true, bytes, end: after };
	if (after <= end) return null;
	return { clear: false, bytes: bytes.subarray(end - offset), end: after };
};
