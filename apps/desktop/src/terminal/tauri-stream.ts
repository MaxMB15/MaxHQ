// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Terminal bytes over the app's backend, with no JSON either way. The page
// has one channel for all its terminals, and every message on it is one
// record: the window's number for the terminal (u32), the offset of the
// bytes in all it printed (u64), then the bytes, big-endian. A record with
// no bytes is a reset. An attach answers with the terminal's number, and a
// record may come before that answer does, so records for a number not yet
// known wait for it. What you type goes as a raw body, with the terminal
// named in a header, and a second header when it came from a field that
// hides it. Nothing here keeps it.

import { Channel, invoke } from "@tauri-apps/api/core";

import type { TerminalId } from "@maxhq/shared/bindings";
import type { TerminalSize } from "@maxhq/shared/terminal/requests";
import type { TerminalEvent, TerminalStream } from "@maxhq/shared/terminal/stream";

/** The bytes before a record's output. */
const HEAD = 12;

/** The header that names the terminal input goes to. */
const TERMINAL_HEADER = "maxhq-terminal";

/** The header that says input came from a field that hides it. */
const SECRET_HEADER = "maxhq-secret";

/** How many records wait for an attach's answer, per number. */
const MOST_WAITING = 256;

/** A record off the channel. */
export type TerminalRecord = { readonly number: number; readonly event: TerminalEvent };

/**
 * Reads one record, which the IPC hands over as an `ArrayBuffer`, or as a
 * plain array of numbers when it falls back to messages. Null when it's too
 * short to be one.
 */
export const readRecord = (body: unknown): TerminalRecord | null => {
	const bytes = Array.isArray(body)
		? Uint8Array.from(body as number[])
		: ArrayBuffer.isView(body)
			? new Uint8Array(body.buffer, body.byteOffset, body.byteLength)
			: body instanceof ArrayBuffer
				? new Uint8Array(body)
				: null;
	if (!bytes || bytes.length < HEAD) return null;
	const head = new DataView(bytes.buffer, bytes.byteOffset, HEAD);
	const number = head.getUint32(0);
	const offset = Number(head.getBigUint64(4));
	const output = bytes.subarray(HEAD);
	return {
		number,
		event:
			output.length === 0
				? { type: "reset", from: offset }
				: { type: "output", offset, bytes: output },
	};
};

/** What the backend says when a call fails, as a sentence. */
const failure = (error: unknown): Error =>
	error instanceof Error ? error : new Error(String(error));

/**
 * The terminal stream over the backend. Resizing is a request like any
 * other, so it comes from the source's own calls.
 */
export const tauriStream = (
	resize: (terminal: TerminalId, size: TerminalSize) => unknown
): TerminalStream => {
	const terminals = new Map<number, TerminalId>();
	const waiting = new Map<number, TerminalEvent[]>();
	const listeners = new Map<TerminalId, Set<(event: TerminalEvent) => void>>();
	let channel: Channel<unknown> | null = null;

	const emit = (terminal: TerminalId, event: TerminalEvent) => {
		for (const listener of [...(listeners.get(terminal) ?? [])]) listener(event);
	};

	const deliver = (body: unknown) => {
		const record = readRecord(body);
		if (!record) return;
		const terminal = terminals.get(record.number);
		if (terminal) return emit(terminal, record.event);
		const held = waiting.get(record.number) ?? [];
		if (held.length < MOST_WAITING) held.push(record.event);
		waiting.set(record.number, held);
	};

	// Made on the first attach, since the backend keeps the first one it's
	// sent for the page's life.
	const channelOf = () => {
		channel ??= new Channel<unknown>(deliver);
		return channel;
	};

	return {
		attach: async (terminal, after) => {
			let number: number;
			try {
				number = await invoke<number>("terminal_attach", {
					channel: channelOf(),
					terminal,
					after,
				});
			} catch (error) {
				throw failure(error);
			}
			terminals.set(number, terminal);
			const held = waiting.get(number);
			waiting.delete(number);
			for (const event of held ?? []) emit(terminal, event);
		},
		detach: (terminal) => {
			invoke("terminal_detach", { terminal }).catch((error: unknown) =>
				console.warn("Couldn't detach a terminal", error)
			);
		},
		resize: (terminal, size) => void resize(terminal, size),
		input: (terminal, bytes, options) => {
			const headers: Record<string, string> = { [TERMINAL_HEADER]: terminal };
			if (options?.secret === true) headers[SECRET_HEADER] = "1";
			// What you type is never logged, so a failure names only the terminal.
			invoke("terminal_input", bytes, { headers }).catch(() =>
				console.warn("Couldn't send input to terminal", terminal)
			);
		},
		listen: (terminal, listener) => {
			let set = listeners.get(terminal);
			if (!set) {
				set = new Set();
				listeners.set(terminal, set);
			}
			set.add(listener);
			return () => {
				set.delete(listener);
				if (set.size === 0 && listeners.get(terminal) === set) listeners.delete(terminal);
			};
		},
	};
};
