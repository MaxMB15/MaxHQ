// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Task, Weekday } from "../bindings";
import { MIN, NOW } from "../test/build";
import {
	addressHint,
	deliveryLabel,
	deliveryTone,
	GITHUB_EVENTS,
	hiddenHookUrl,
	lastRunLine,
	nextRunLine,
	relativeLine,
	triggerLine,
	waitLine,
	webhookLine,
} from "./format";

const RULES = {
	filters: [],
	replay_key: null,
	coalesce_key: null,
	coalesce_secs: null,
	max_age_minutes: null,
	daily_runs: null,
};

const task = (over: Partial<Task>): Task => ({
	id: "t",
	name: "T",
	prompt: "p",
	trigger: { type: "every", minutes: 15 },
	target: { type: "agent", device: "d", agent: "a" },
	enabled: true,
	permission: "perm",
	made_by: { type: "user" },
	next_run: null,
	last_run: null,
	...over,
});

const at = (days: Weekday[]) =>
	({ type: "at", time: { hour: 9, minute: 5 }, days, zone: "Europe/Amsterdam" }) as const;

describe("what makes a task run", () => {
	it("says the interval in minutes or hours", () => {
		expect(triggerLine({ type: "every", minutes: 15 })).toBe("Every 15 min");
		expect(triggerLine({ type: "every", minutes: 60 })).toBe("Every hour");
		expect(triggerLine({ type: "every", minutes: 180 })).toBe("Every 3 hours");
		expect(triggerLine({ type: "every", minutes: 90 })).toBe("Every 90 min");
	});

	it("says the days and time, and the zone only when it isn't yours", () => {
		expect(triggerLine(at([]), "Europe/Amsterdam")).toBe("Daily at 09:05");
		expect(
			triggerLine(
				at(["monday", "tuesday", "wednesday", "thursday", "friday"]),
				"Europe/Amsterdam"
			)
		).toBe("Weekdays at 09:05");
		expect(triggerLine(at(["wednesday", "monday"]), "UTC")).toBe(
			"Mon, Wed at 09:05 Europe/Amsterdam"
		);
	});

	it("names the webhook and the GitHub events", () => {
		expect(
			triggerLine({ type: "hook", on: true, signature: null, rules: RULES, hold: false })
		).toBe("On webhook");
		expect(
			triggerLine({
				type: "github_events",
				repo: "owner/repo",
				events: ["pull_request", "push"],
				rules: RULES,
			})
		).toBe("On pull requests and pushes in owner/repo");
	});

	it("says how long a task that coalesces waits", () => {
		const all = { ...RULES, coalesce_key: "all" };
		expect(waitLine(RULES, "event")).toBeNull();
		expect(waitLine(all, "request")).toBe("Waits 2 min after the first request");
		const restart = { ...all, coalesce_secs: 900, coalesce_restart: true };
		expect(waitLine(restart, "event")).toBe("Waits 15 min after the last event, 2 h at most");
		expect(waitLine({ ...restart, coalesce_secs: 45, coalesce_max_secs: 5400 }, "event")).toBe(
			"Waits 45 s after the last event, 90 min at most"
		);
		expect(
			triggerLine({
				type: "github_events",
				repo: "owner/repo",
				events: ["pull_request"],
				rules: restart,
			})
		).toBe("On pull requests in owner/repo. Waits 15 min after the last event, 2 h at most");
	});
});

describe("when a task runs next", () => {
	it("says how long until the next run", () => {
		expect(nextRunLine(task({ next_run: NOW + 5 * MIN }), NOW)).toBe("Next run in 5m");
		expect(nextRunLine(task({ next_run: NOW + 3 * 60 * MIN }), NOW)).toBe("Next run in 3h");
		expect(nextRunLine(task({ next_run: NOW + 30_000 }), NOW)).toBe(
			"Next run in under a minute"
		);
		expect(nextRunLine(task({}), NOW)).toBe("Not scheduled");
	});

	it("says why it waits instead", () => {
		expect(nextRunLine(task({ enabled: false, next_run: NOW + MIN }), NOW)).toBe("Paused");
		expect(nextRunLine(task({ enabled: false, approval_asked: NOW }), NOW)).toBe(
			"Waits for your approval"
		);
		const hook = (on: boolean) =>
			task({ trigger: { type: "hook", on, signature: null, rules: RULES, hold: false } });
		expect(nextRunLine(hook(true), NOW)).toBe("Listening");
		expect(nextRunLine(hook(false), NOW)).toBe("Webhook off");
		expect(
			nextRunLine(
				task({
					trigger: { type: "github_events", repo: "o/r", events: ["push"], rules: RULES },
				}),
				NOW
			)
		).toBe("Watching o/r");
	});

	it("counts days far out", () => {
		expect(relativeLine(NOW + 3 * 24 * 60 * MIN, NOW)).toBe("in 3d");
	});
});

describe("how a GitHub events task hears of events", () => {
	it("says whether a webhook brings them at once, and when it last did", () => {
		const watching = task({
			trigger: { type: "github_events", repo: "o/r", events: ["push"], rules: RULES },
		});
		const heard = (webhook: { instant: boolean; last_delivery: number | null } | null) =>
			webhookLine(watching, { webhooks: webhook ? [{ task: "t", ...webhook }] : [] }, NOW);
		expect(heard({ instant: true, last_delivery: NOW - 5 * MIN })).toBe(
			"Instant through a webhook. Last delivery 5 min ago"
		);
		expect(heard({ instant: false, last_delivery: NOW - 3 * 60 * MIN })).toBe(
			"Checks every minute. Last delivery 3 h ago"
		);
		expect(heard({ instant: false, last_delivery: null })).toBe("Checks every minute");
		// Without a webhook on its repository, it polls.
		expect(heard(null)).toBe("Checks every minute");
		// An older maxd doesn't say, a paused task hears nothing, and other tasks don't listen.
		expect(webhookLine(watching, {}, NOW)).toBeNull();
		expect(webhookLine({ ...watching, enabled: false }, { webhooks: [] }, NOW)).toBeNull();
		expect(webhookLine(task({}), { webhooks: [] }, NOW)).toBeNull();
	});
});

describe("how the last run went", () => {
	it("says the outcome and when", () => {
		expect(lastRunLine(null, NOW)).toBe("No runs yet");
		expect(lastRunLine({ at: NOW - MIN, outcome: "running" }, NOW)).toBe("Running now");
		expect(lastRunLine({ at: NOW - 20 * MIN, outcome: "failed" }, NOW)).toBe(
			"Failed 20 min ago"
		);
	});
});

describe("what came of a request", () => {
	it("labels and tones each outcome", () => {
		expect(deliveryLabel("rejected_signature")).toBe("Bad signature");
		expect(deliveryTone("accepted")).toBe("good");
		expect(deliveryTone("rejected_signature")).toBe("bad");
		expect(deliveryTone("duplicate")).toBe("warn");
		expect(deliveryTone("filtered")).toBe("plain");
	});
});

describe("the GitHub events", () => {
	it("go by their webhook names", () => {
		expect(GITHUB_EVENTS.map((e) => e.event)).toContain("pull_request");
		expect(GITHUB_EVENTS.every((e) => !e.event.endsWith("Event"))).toBe(true);
	});
});

describe("a webhook's address", () => {
	it("says how a device is reached, and on Funnel whether it works", () => {
		expect(addressHint({ kind: "local" }, { state: "on", url: "http://127.0.0.1:41234" })).toBe(
			"This device only"
		);
		expect(
			addressHint({ kind: "own", base: "https://hooks.example.com" }, { state: "off" })
		).toBe("Your own address");
		expect(addressHint({ kind: "funnel" }, { state: "needs_setup", steps: [] })).toBe(
			"Funnel needs setting up"
		);
	});

	it("hides the token, and shows only the path before the address works", () => {
		expect(hiddenHookUrl("task-1", { state: "on", url: "https://devbox.example.ts.net" })).toBe(
			"https://devbox.example.ts.net/maxhq/task-1/••••••"
		);
		expect(hiddenHookUrl("task-1", { state: "off" })).toBe("/maxhq/task-1/••••••");
	});
});
