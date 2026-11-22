// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { drawerShows } from "./shown";

const TABS = [{ id: "session-1" }, { id: "shell-2" }, { id: "run-3" }];

describe("what the agent's drawer shows", () => {
	it("shows nothing while it's closed", () => {
		expect(drawerShows(false, TABS, "session-1", undefined)).toEqual([]);
		expect(drawerShows(true, [], undefined, undefined)).toEqual([]);
	});

	it("shows the tab you picked, or the newest", () => {
		expect(drawerShows(true, TABS, "session-1", undefined)).toEqual(["session-1"]);
		expect(drawerShows(true, TABS, undefined, undefined)).toEqual(["run-3"]);
		expect(drawerShows(true, TABS, "gone-9", undefined)).toEqual(["run-3"]);
	});

	it("shows the tab beside it too, while that's still a tab of its own", () => {
		expect(drawerShows(true, TABS, "session-1", "shell-2")).toEqual(["session-1", "shell-2"]);
		expect(drawerShows(true, TABS, "session-1", "session-1")).toEqual(["session-1"]);
		expect(drawerShows(true, TABS, "session-1", "gone-9")).toEqual(["session-1"]);
	});
});
