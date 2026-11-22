// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { TimelineItem } from "../bindings";
import { OBJECT_MARK, type OutputRef, outputMarker } from "../pills/draft";

import {
	fillOutput,
	lastLines,
	OUTPUT_LINES,
	runStatus,
	startedOutput,
	type UserRunItem,
	userRunOf,
	withStatus,
} from "./output";

const AT = 1_000_000;

const started = startedOutput({
	command: "pnpm test",
	deviceName: "Mac",
	cwd: "~/code/example-app",
	terminal: "t1",
	at: AT,
});

const ran = (over: Partial<UserRunItem> = {}): UserRunItem => ({
	id: "i1",
	agent: "a1",
	turn: null,
	parent: null,
	ordinal: 1,
	status: "failed",
	started_at: AT,
	completed_at: AT + 4200,
	kind: "user_run",
	terminal: "t1",
	command: "pnpm test",
	cwd: "~/code/example-app",
	device: "dev-mac",
	private: false,
	process: { state: "exited", code: 1, signal: null },
	duration_ms: 4200,
	lines: 3,
	output: "\u001b[31mFAIL\u001b[0m src/cache.test.ts\nTests 1 failed\ndone\n",
	output_truncated: false,
	moved: false,
	...over,
});

describe("lastLines", () => {
	it("gives the last lines and counts them all", () => {
		expect(lastLines("a\nb\nc\n", 2)).toEqual({ text: "b\nc", total: 3 });
		expect(lastLines("", 2)).toEqual({ text: "", total: 0 });
		expect(lastLines("one", 200)).toEqual({ text: "one", total: 1 });
	});
});

describe("startedOutput", () => {
	it("names a running command and its terminal", () => {
		expect(started).toMatchObject({
			type: "output",
			command: "pnpm test",
			device_name: "Mac",
			running: true,
			exit_code: null,
			terminal: "t1",
			text: "",
		});
	});
});

describe("userRunOf and runStatus", () => {
	it("finds the item for a terminal", () => {
		const item = ran();
		const state = { items: new Map<string, TimelineItem>([["i1", item]]), order: ["i1"] };
		expect(userRunOf(state, "t1")).toBe(item);
		expect(userRunOf(state, "t2")).toBeUndefined();
	});

	it("reads the terminal first, then the item", () => {
		const item = ran();
		expect(runStatus({ status: { state: "running" } }, item)).toEqual({ state: "running" });
		expect(runStatus(undefined, item)).toEqual(item.process);
		expect(runStatus(undefined, undefined)).toBeNull();
	});
});

describe("withStatus", () => {
	it("carries the signal and whether you stopped it", () => {
		expect(
			withStatus(started, { state: "exited", code: null, signal: 2, stopped: true })
		).toMatchObject({ running: false, exit_code: null, signal: 2, stopped: true });
		expect(withStatus(started, { state: "exited", code: 1, signal: null })).toMatchObject({
			running: false,
			exit_code: 1,
			signal: null,
			stopped: false,
		});
	});

	it("says it runs, and leaves the pill as it is without a status", () => {
		const ended = withStatus(started, { state: "exited", code: 0, signal: null });
		expect(withStatus(ended, { state: "running" })).toMatchObject({
			running: true,
			exit_code: null,
			signal: null,
			stopped: false,
		});
		expect(withStatus(started, null)).toBe(started);
	});
});

describe("fillOutput", () => {
	it("takes an ended command's last lines without their colors", () => {
		expect(fillOutput(started, { item: ran(), now: AT + 9000 })).toMatchObject({
			running: false,
			exit_code: 1,
			text: "FAIL src/cache.test.ts\nTests 1 failed\ndone",
			lines_total: 3,
			duration_ms: 4200,
		});
	});

	it("takes no more than the last 200 lines", () => {
		const output = Array.from({ length: 214 }, (_, n) => `line ${n + 1}`).join("\n");
		const filled = fillOutput(started, { item: ran({ output, lines: 214 }), now: AT });
		expect(filled.text.split("\n")).toHaveLength(OUTPUT_LINES);
		expect(filled.text.startsWith("line 15\n")).toBe(true);
		expect(filled.lines_total).toBe(214);
	});

	it("takes the live terminal's lines while the command runs", () => {
		const filled = fillOutput(started, {
			item: ran({ process: { state: "running" }, status: "running" }),
			terminal: { status: { state: "running" } },
			live: { text: "RUN v3\n✓ router", total: 40 },
			now: AT + 8000,
		});
		expect(filled).toMatchObject({
			running: true,
			exit_code: null,
			text: "RUN v3\n✓ router",
			lines_total: 40,
			duration_ms: 8000,
		});
	});

	it("follows the terminal when it ended before its item says so", () => {
		const filled = fillOutput(started, {
			item: ran({ process: { state: "running" }, output: "partial\n", lines: 1 }),
			terminal: { status: { state: "exited", code: 0, signal: null } },
			now: AT + 5000,
		});
		expect(filled).toMatchObject({ running: false, exit_code: 0, text: "partial" });
	});

	it("keeps lines picked in the drawer as they were picked", () => {
		const picked: OutputRef = { ...started, lines: { first: 6, last: 8 }, text: "six" };
		expect(fillOutput(picked, { item: ran(), now: AT })).toBe(picked);
	});
});

describe("outputMarker", () => {
	it("names the output the way core's marker does", () => {
		const running: OutputRef = {
			...started,
			command: "pnpm dev",
			lines: { first: 6, last: 8 },
		};
		expect(outputMarker(running, 1)).toBe("[Output 1: pnpm dev, running, lines 6-8]");
		expect(outputMarker({ ...started, running: false, exit_code: 1 })).toBe(
			"[Output: pnpm test, exit 1]"
		);
		expect(outputMarker({ ...started, running: false, exit_code: null }, 2)).toBe(
			"[Output 2: pnpm test]"
		);
	});

	it("puts the command on one line, without brackets, and cuts it at 80 characters", () => {
		const command = `echo [a]${OBJECT_MARK}\n\t  b`;
		expect(outputMarker({ ...started, command }, 1)).toBe("[Output 1: echo a b, running]");
		const long = "x".repeat(90);
		expect(outputMarker({ ...started, command: long }, 1)).toBe(
			`[Output 1: ${"x".repeat(80)}…, running]`
		);
		expect(outputMarker({ ...started, command: "[]" }, 1)).toBe("[Output 1: command, running]");
	});
});
