// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { TerminalEvent } from "@maxhq/shared/terminal/stream";

import { readRecord, tauriStream } from "./tauri-stream";

const mocks = vi.hoisted(() => {
	class FakeChannel {
		static made: FakeChannel[] = [];
		constructor(readonly onmessage: (body: unknown) => void) {
			FakeChannel.made.push(this);
		}
	}
	return { FakeChannel, invoke: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({ Channel: mocks.FakeChannel, invoke: mocks.invoke }));

/** A record as the backend frames it. */
const record = (number: number, offset: number, text = "") => {
	const output = new TextEncoder().encode(text);
	const bytes = new Uint8Array(12 + output.length);
	const head = new DataView(bytes.buffer);
	head.setUint32(0, number);
	head.setBigUint64(4, BigInt(offset));
	bytes.set(output, 12);
	return bytes.buffer;
};

const text = (event: TerminalEvent | undefined) =>
	event?.type === "output" ? new TextDecoder().decode(event.bytes) : event;

describe("reading a terminal record", () => {
	it("reads the number, the offset and the output, in a buffer or a plain array", () => {
		const read = readRecord(record(7, 2 ** 40, "ok\r\n"));
		expect(read?.number).toBe(7);
		expect(read?.event).toMatchObject({ type: "output", offset: 2 ** 40 });
		expect(text(read?.event)).toBe("ok\r\n");
		const plain = readRecord([...new Uint8Array(record(1, 3, "x"))]);
		expect(plain?.number).toBe(1);
		expect(text(plain?.event)).toBe("x");
	});

	it("reads a record with no output as a reset, and skips one too short", () => {
		expect(readRecord(record(2, 40))).toEqual({
			number: 2,
			event: { type: "reset", from: 40 },
		});
		expect(readRecord(new Uint8Array(11).buffer)).toBeNull();
		expect(readRecord("junk")).toBeNull();
	});
});

describe("the stream over the backend", () => {
	it("holds records that come before the attach answers, then hands them over in order", async () => {
		let answer: (number: number) => void = () => {};
		mocks.invoke.mockReturnValueOnce(new Promise<number>((resolve) => (answer = resolve)));
		const stream = tauriStream(() => {});
		const got: (string | TerminalEvent | undefined)[] = [];
		stream.listen("t-1", (event) => got.push(text(event)));
		const attached = stream.attach("t-1", null);
		const [channel] = mocks.FakeChannel.made;
		channel!.onmessage(record(4, 0, "one "));
		channel!.onmessage(record(4, 4, "two "));
		expect(got).toEqual([]);
		answer(4);
		await attached;
		channel!.onmessage(record(4, 8, "three"));
		expect(got).toEqual(["one ", "two ", "three"]);
		expect(mocks.invoke).toHaveBeenCalledWith("terminal_attach", {
			channel,
			terminal: "t-1",
			after: null,
		});
	});

	it("sends what you type as a raw body that names the terminal in a header", () => {
		mocks.invoke.mockResolvedValueOnce(null);
		const bytes = new TextEncoder().encode("ls\r");
		tauriStream(() => {}).input("t-1", bytes);
		expect(mocks.invoke).toHaveBeenLastCalledWith("terminal_input", bytes, {
			headers: { "maxhq-terminal": "t-1" },
		});
	});
});
