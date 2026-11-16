// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { setTask, taskCount } from "./tasks";

describe("task boxes", () => {
	it("counts bullet, numbered, nested and quoted tasks in order", () => {
		const text = "- [ ] one\n  - [x] two\n1. [X] three\n> - [ ] four\n- not a task";
		expect(taskCount(text)).toBe(4);
		expect(setTask(text, 3, true)).toBe(
			"- [ ] one\n  - [x] two\n1. [X] three\n> - [x] four\n- not a task"
		);
	});

	it("ticks and clears a box", () => {
		expect(setTask("- [ ] one\n- [ ] two", 1, true)).toBe("- [ ] one\n- [x] two");
		expect(setTask("- [X] one", 0, false)).toBe("- [ ] one");
	});

	it("skips boxes inside fenced code", () => {
		const text =
			"```md\n- [ ] code\n```\n- [ ] real\n~~~~\n- [ ] more code\n~~~\n~~~~\n- [ ] last";
		expect(taskCount(text)).toBe(2);
		expect(setTask(text, 1, true)).toBe(
			"```md\n- [ ] code\n```\n- [ ] real\n~~~~\n- [ ] more code\n~~~\n~~~~\n- [x] last"
		);
	});

	it("leaves the text alone for an index past the last box", () => {
		expect(setTask("- [ ] one", 1, true)).toBe("- [ ] one");
		expect(setTask("- [ ] one", -1, true)).toBe("- [ ] one");
	});
});
