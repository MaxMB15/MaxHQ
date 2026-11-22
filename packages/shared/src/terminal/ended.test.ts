// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { endedText, signalName } from "./ended";

describe("signalName", () => {
	it("names the common signals", () => {
		expect(
			[1, 2, 3, 4, 6, 8, 9, 11, 13, 14, 15].map((signal) => [signal, signalName(signal)])
		).toEqual([
			[1, "hangup"],
			[2, "interrupt"],
			[3, "quit"],
			[4, "illegal instruction"],
			[6, "abort"],
			[8, "floating point error"],
			[9, "killed"],
			[11, "segmentation fault"],
			[13, "broken pipe"],
			[14, "alarm"],
			[15, "terminated"],
		]);
	});

	it("names no other", () => {
		expect(signalName(10)).toBeNull();
		expect(signalName(31)).toBeNull();
	});
});

describe("endedText", () => {
	const exited = (code: number | null, signal: number | null, stopped?: boolean) => ({
		state: "exited" as const,
		code,
		signal,
		...(stopped === undefined ? {} : { stopped }),
	});

	it("gives the exit code and how long it took", () => {
		expect(endedText(exited(1, null), 4200)).toBe("exit 1 · 4.2 s");
		expect(endedText(exited(0, null), null)).toBe("exit 0");
	});

	it("says you stopped it, in whole seconds, whatever ended it", () => {
		expect(endedText(exited(null, 2, true), 33_400)).toBe("Stopped after 33 s");
		expect(endedText(exited(130, null, true), 4200)).toBe("Stopped after 4 s");
		expect(endedText(exited(null, 9, true), null)).toBe("Stopped");
	});

	it("names the signal that ended it when you didn't", () => {
		expect(endedText(exited(null, 9), 4200)).toBe("Ended by signal 9 (killed) · 4.2 s");
		expect(endedText(exited(null, 9, false), null)).toBe("Ended by signal 9 (killed)");
		expect(endedText(exited(null, 10), null)).toBe("Ended by signal 10");
	});

	it("says Stopped when neither a code nor a signal says how", () => {
		expect(endedText(exited(null, null), null)).toBe("Stopped");
	});
});
