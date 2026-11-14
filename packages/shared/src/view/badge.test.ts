// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";

import type { Agent, Inbox } from "../bindings";
import { agent, attention, entities, MIN, NOW, project, workspace } from "../test/build";

import {
	BADGE_RECHECK_MS,
	badgeCount,
	type BadgePrefs,
	DEFAULT_BADGE_PREFS,
	readBadgePrefs,
	switchReason,
	watchBadge,
	writeBadgePrefs,
} from "./badge";
import { DEFAULT_INBOX } from "./inbox";
import { applySnapshot, type ViewState } from "./replica";
import { createViewStore } from "./store";

const inbox = (over: Partial<Inbox>): Inbox => ({ ...DEFAULT_INBOX, ...over });

const viewOf = (agents: Agent[], hidden: string[] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				projects: [project("p1", "notes-app"), project("p2", "hidden-app")],
				workspaces: [
					workspace({ id: "w1", project: "p1" }),
					workspace({ id: "w2", project: "p2" }),
				],
				attention: [
					attention({ id: "e-ask", agent: "ask" }),
					attention({ id: "e-q", agent: "question", kind: "question" }),
					attention({ id: "e-err", agent: "prompt-err", kind: "error" }),
				],
				agents,
			}),
			{ entity: "hidden_projects" as const, device: "dev-mac", projects: hidden },
		],
	});

const ask = agent({ id: "ask", activity: { state: "needs_input", attention: "e-ask" } });
const question = agent({
	id: "question",
	activity: { state: "needs_input", attention: "e-q" },
});
const done = agent({ id: "done", activity: { state: "done_unread" } });
const crashed = agent({ id: "crashed", activity: { state: "error", reason: "exit 1" } });
const promptError = agent({
	id: "prompt-err",
	activity: { state: "needs_input", attention: "e-err" },
});
const idle = agent({ id: "idle" });
const busy = agent({ id: "busy", activity: { state: "busy" } });

const only = (...counts: BadgePrefs["counts"]): BadgePrefs => ({ show: true, counts });

describe("the badge count", () => {
	const view = viewOf([ask, question, done, crashed, promptError, idle, busy]);

	it("counts every agent that needs you once, by default", () => {
		expect(badgeCount(view, NOW, DEFAULT_BADGE_PREFS)).toBe(5);
	});

	it("counts only the reasons that are on", () => {
		expect(badgeCount(view, NOW, only("waiting"))).toBe(2);
		expect(badgeCount(view, NOW, only("finished"))).toBe(1);
		// A crash and a prompt that is an error both count as failed.
		expect(badgeCount(view, NOW, only("failed"))).toBe(2);
		expect(badgeCount(view, NOW, only())).toBe(0);
	});

	it("shows nothing while the badge is off", () => {
		expect(badgeCount(view, NOW, { ...DEFAULT_BADGE_PREFS, show: false })).toBe(0);
	});

	it("leaves out settled, snoozed and sub-agents", () => {
		const view = viewOf([
			{ ...done, inbox: inbox({ settled: { at: NOW - MIN, by: "you" } }) },
			{ ...crashed, inbox: inbox({ snoozed_until: NOW + 10 * MIN, snoozed_at: NOW }) },
			{ ...ask, parent: "lead" },
			// A snooze that ran out has ended, even before maxd clears it.
			{ ...question, inbox: inbox({ snoozed_until: NOW - MIN, snoozed_at: NOW - 9 * MIN }) },
		]);
		expect(badgeCount(view, NOW, DEFAULT_BADGE_PREFS)).toBe(1);
	});

	it("leaves out agents in hidden projects", () => {
		const view = viewOf([done, { ...crashed, workspace: "w2" }], ["p2"]);
		expect(badgeCount(view, NOW, DEFAULT_BADGE_PREFS)).toBe(1);
	});
});

describe("the badge settings", () => {
	it("read back what was written", () => {
		const prefs = only("waiting", "failed");
		expect(readBadgePrefs(writeBadgePrefs(prefs))).toEqual(prefs);
	});

	it("default whatever doesn't read", () => {
		expect(readBadgePrefs(null)).toEqual(DEFAULT_BADGE_PREFS);
		expect(readBadgePrefs("{nope")).toEqual(DEFAULT_BADGE_PREFS);
		expect(readBadgePrefs(JSON.stringify({ show: false }))).toEqual(DEFAULT_BADGE_PREFS);
		expect(
			readBadgePrefs(JSON.stringify({ version: 1, show: "no", counts: ["failed", "x"] }))
		).toEqual({ show: true, counts: ["failed"] });
	});

	it("switch one reason and keep settings order", () => {
		expect(switchReason(["failed"], "waiting", true)).toEqual(["waiting", "failed"]);
		expect(switchReason(["waiting", "failed"], "failed", false)).toEqual(["waiting"]);
	});
});

describe("watching the badge", () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it("tells the count at once and then only when it changes", () => {
		vi.useFakeTimers();
		const view = createViewStore();
		view.setState({ view: viewOf([done]) });
		const prefs = createStore<BadgePrefs>(() => DEFAULT_BADGE_PREFS);
		let now = NOW;
		const snoozed = {
			...crashed,
			inbox: inbox({ snoozed_until: NOW + MIN, snoozed_at: NOW }),
		};
		const set = vi.fn();
		const stop = watchBadge({ view, prefs, set, now: () => now });
		expect(set.mock.calls).toEqual([[1]]);

		view.setState({ view: viewOf([done, idle]) });
		expect(set.mock.calls).toEqual([[1]]);

		view.setState({ view: viewOf([done, ask, snoozed]) });
		prefs.setState({ counts: ["finished"] });
		prefs.setState({ show: false });
		expect(set.mock.calls).toEqual([[1], [2], [1], [0]]);

		prefs.setState(DEFAULT_BADGE_PREFS);
		now = NOW + 2 * MIN;
		vi.advanceTimersByTime(BADGE_RECHECK_MS);
		expect(set.mock.calls).toEqual([[1], [2], [1], [0], [2], [3]]);

		stop();
		view.setState({ view: viewOf([]) });
		expect(set).toHaveBeenCalledTimes(6);
	});
});
