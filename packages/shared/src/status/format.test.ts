// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { MIN, NOW } from "../test/build";

import {
	formatAgo,
	formatAgoShort,
	formatContext,
	formatDollars,
	formatDuration,
	formatWhen,
	isClipped,
	listInWords,
	showHidden,
	sentence,
} from "./format";

describe("format", () => {
	it("writes durations the way the hovers do", () => {
		expect(formatDuration(12_000)).toBe("12 s");
		expect(formatDuration(72_000)).toBe("1 min 12 s");
		expect(formatDuration(38 * MIN)).toBe("38 min");
		expect(formatDuration(142 * MIN)).toBe("2 h 22 min");
		expect(formatDuration(76 * 60 * MIN)).toBe("3 d 4 h");
	});

	it("writes how long ago, long and short", () => {
		expect(formatAgo(NOW - 2000, NOW)).toBe("2 s ago");
		expect(formatAgo(NOW - 131 * MIN - 20_000, NOW)).toBe("2 h 11 min ago");
		expect(formatAgoShort(NOW - 4 * MIN, NOW)).toBe("4m");
		expect(formatAgoShort(NOW - 3 * 60 * MIN, NOW)).toBe("3h");
		expect(formatAgoShort(NOW - 49 * 60 * MIN, NOW)).toBe("2d");
	});

	it("writes a clock time today and a weekday later in the week", () => {
		expect(formatWhen(new Date(2026, 9, 25, 17, 10).getTime(), NOW)).toBe("17:10");
		expect(formatWhen(new Date(2026, 9, 29, 9, 0).getTime(), NOW)).toBe("Thu 09:00");
	});

	it("writes cents as dollars", () => {
		expect(formatDollars(1240)).toBe("$12.40");
		expect(formatDollars(5000)).toBe("$50");
		expect(formatDollars(5)).toBe("$0.05");
	});

	it("writes context and lists in words", () => {
		expect(formatContext(118_000, 200_000)).toBe("118k of 200k");
		expect(formatContext(400_000, 1_000_000)).toBe("400k of 1M");
		expect(listInWords(["a", "b", "c"])).toBe("a, b and c");
	});

	it("starts a backend's reason as a sentence without touching names", () => {
		expect(sentence("connection refused.")).toBe("Connection refused");
		expect(sentence("devbox-1 is offline on the tailnet")).toBe(
			"devbox-1 is offline on the tailnet"
		);
		expect(sentence("maxd stopped answering")).toBe("maxd stopped answering");
		expect(sentence("ssh: connect to host devbox")).toBe("ssh: connect to host devbox");
		expect(sentence("couldn't restart Claude")).toBe("Couldn't restart Claude");
		expect(sentence("it's already running")).toBe("It's already running");
		expect(sentence("devbox's maxd is too old")).toBe("devbox's maxd is too old");
		expect(sentence("")).toBe("");
	});

	it("tells a summary maxd cut short", () => {
		expect(isClipped("Run `cargo test…`?")).toBe(true);
		expect(isClipped("Search the web for rust async…?")).toBe(true);
		expect(isClipped("Which branch should the release notes come from…")).toBe(true);
		expect(isClipped("Run `cargo test`?")).toBe(false);
		expect(isClipped("Edit src/main.rs?")).toBe(false);
	});

	it("spells out what text would hide", () => {
		expect(showHidden("rm -rf ~\u202e txt.md")).toBe("rm -rf ~[U+202E] txt.md");
		expect(showHidden("a\u200bb\u2066c\u00add")).toBe("a[U+200B]b[U+2066]c[U+00AD]d");
		expect(showHidden("run\rsafe\u0007")).toBe("run[U+000D]safe[U+0007]");
		expect(showHidden("x\u{e0041}")).toBe("x[U+E0041]");
	});

	it("leaves what shows as itself", () => {
		const shown = "one\ntwo\r\n\tthree caf\u00e9 \u05d0 \u4e2d \u2764\ufe0f \u{1f600}";
		expect(showHidden(shown)).toBe(shown);
	});
});
