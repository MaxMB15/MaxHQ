// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { BINARY_SNIFF } from "@maxhq/shared/agent-view/code-text";
import { describe, expect, it } from "vitest";

import { binaryNote, readText, tooBigNote } from "./code-file";

const bytes = (...parts: (string | number[])[]): ArrayBuffer => {
	const all = parts.flatMap((part) =>
		typeof part === "string" ? [...new TextEncoder().encode(part)] : part
	);
	return Uint8Array.from(all).buffer;
};

describe("reading a file as text", () => {
	it("splits lines of any ending, without an empty one for the last break", () => {
		expect(readText(bytes("a\r\nb\nc\n"))).toEqual({
			kind: "text",
			lines: ["a", "b", "c"],
			bytes: 7,
		});
		expect(readText(bytes(""))).toMatchObject({ lines: [""] });
		expect(readText(bytes("a\n\n"))).toMatchObject({ lines: ["a", ""] });
	});

	it("puts a replacement character where the bytes aren't UTF-8", () => {
		expect(readText(bytes("caf", [0xe9], "!"))).toMatchObject({ lines: ["caf�!"] });
	});

	it("calls a file with a zero byte early on binary, and one later on text", () => {
		expect(readText(bytes("ok", [0], "more"))).toEqual({ kind: "binary" });
		const late = bytes("x".repeat(BINARY_SNIFF), [0]);
		expect(readText(late).kind).toBe("text");
	});

	it("says in plain words why a file isn't shown", () => {
		expect(binaryNote("/tmp/example/data.log")).toBe(
			"data.log looks like a binary file, so it isn't shown as text. Open it in an editor with the Open in button above."
		);
		expect(tooBigNote("src/huge.ts", "9.0 MB")).toBe(
			"huge.ts is 9.0 MB, too big to show here. Open it in an editor with the Open in button above."
		);
	});
});
