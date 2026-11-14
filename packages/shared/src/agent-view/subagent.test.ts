// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { endedLine, runningLine, type Work } from "./subagent";

const none: Work = { tools: null, partial: false, now: null };
const three: Work = { tools: 3, partial: false, now: null };
const atLeast: Work = { tools: 3, partial: true, now: null };

describe("runningLine", () => {
	it("counts up from the start when it knows the start", () => {
		expect(runningLine(100, none)).toEqual({ words: "Sub-agent, running for ", since: 100 });
		expect(runningLine(100, three)).toEqual({ words: "Sub-agent, 3 tool calls, ", since: 100 });
	});

	it("has no clock without a start", () => {
		expect(runningLine(null, none)).toEqual({ words: "Sub-agent, running", since: null });
		expect(runningLine(null, three)).toEqual({ words: "Sub-agent, 3 tool calls", since: null });
	});

	it("says at least while older steps haven't loaded", () => {
		expect(runningLine(null, atLeast).words).toBe("Sub-agent, at least 3 tool calls");
	});
});

describe("endedLine", () => {
	const ended = (
		status: "completed" | "failed" | "cancelled" | "interrupted" | "declined",
		took: number | null = null
	) => ({
		status,
		started_at: took === null ? null : 1000,
		completed_at: took === null ? null : 1000 + took,
	});

	it("says what a finished one did and how long it took", () => {
		expect(endedLine(ended("completed", 65_000), three)).toBe(
			"Sub-agent, 3 tool calls, took 1 min 5 s"
		);
		expect(endedLine(ended("completed", 65_000), none)).toBe("Sub-agent, took 1 min 5 s");
		expect(endedLine(ended("completed"), three)).toBe("Sub-agent, 3 tool calls");
		expect(endedLine(ended("completed"), none)).toBe("Sub-agent, done");
	});

	it("says how far a failed or stopped one got", () => {
		expect(endedLine(ended("failed"), three)).toBe("Sub-agent, failed after 3 tool calls");
		expect(endedLine(ended("failed"), none)).toBe("Sub-agent, failed");
		for (const status of ["cancelled", "interrupted", "declined"] as const) {
			expect(endedLine(ended(status), atLeast)).toBe(
				"Sub-agent, stopped after at least 3 tool calls"
			);
		}
	});
});
