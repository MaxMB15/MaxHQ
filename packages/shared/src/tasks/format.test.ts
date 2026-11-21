// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Task, TaskDelivery, Weekday } from "../bindings";
import { MIN, NOW } from "../test/build";
import {
	addressHint,
	addressNote,
	deliveryFacts,
	deliveryLabel,
	deliveryTone,
	fieldKind,
	fieldsNote,
	GITHUB_EVENTS,
	hiddenHookUrl,
	lastRunLine,
	makerLine,
	needsSecret,
	nextRunLine,
	ownAddressHint,
	relativeLine,
	triggerLine,
	waitLine,
	webhookLine,
	zoneHint,
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

	it("says when a task runs in its zone under a picker on another clock", () => {
		const morning = {
			time: "23:00",
			days: new Set<Weekday>(["wednesday", "monday"]),
			zone: "Europe/Amsterdam",
		};
		expect(zoneHint(morning, "Europe/Amsterdam")).toBe(
			"In Europe/Amsterdam. No days picked runs it every day."
		);
		expect(zoneHint(morning, "America/New_York")).toBe(
			"In Europe/Amsterdam: Mon, Wed at 23:00. No days picked runs it every day."
		);
		expect(zoneHint({ ...morning, days: new Set() }, "Asia/Tokyo")).toBe(
			"In Europe/Amsterdam: Daily at 23:00. No days picked runs it every day."
		);
		expect(zoneHint({ ...morning, time: "" }, "Asia/Tokyo")).toBe(
			"In Europe/Amsterdam. No days picked runs it every day."
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

describe("who made a task", () => {
	it("marks one the dispatcher or an agent made, and none you made", () => {
		expect(makerLine(task({}))).toBeNull();
		expect(makerLine(task({ by_dispatcher: true }))).toBe("Made by the dispatcher");
		const origin = {
			permission: "perm",
			by: { kind: "agent", agent: "a" },
			device: "d",
		} as const;
		expect(makerLine(task({ made_by: { type: "tools", origin } }))).toBe("Made by an agent");
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

describe("how a request came", () => {
	it("names its method, type, size and signature", () => {
		const delivery: TaskDelivery = {
			id: "d",
			task: "t",
			received: NOW,
			method: "POST",
			content_type: "application/json",
			body_bytes: 1200,
			outcome: "accepted",
			signature_verified: true,
			missing_fields: [],
			error: null,
		};
		expect(deliveryFacts(delivery)).toEqual([
			"POST",
			"application/json",
			"1,200 bytes",
			"signed",
		]);
		expect(
			deliveryFacts({ ...delivery, content_type: null, signature_verified: false })
		).toEqual(["POST", "1,200 bytes"]);
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

	it("says who reaches a device's webhooks, and leaves Funnel's setup to its steps", () => {
		const off = { state: "off" } as const;
		expect(addressNote({ funnel: off }, "devbox")).toMatch(/^Only programs on devbox/);
		expect(
			addressNote(
				{
					address: { kind: "own", base: "https://hooks.example.com" },
					port: 41234,
					funnel: off,
				},
				"devbox"
			)
		).toBe(
			"Requests come in through https://hooks.example.com. maxd can't check that it reaches port 41234 on devbox."
		);
		expect(
			addressNote(
				{ address: { kind: "own", base: "https://h.example.com" }, funnel: off },
				"devbox"
			)
		).toMatch(/reaches the hooks port on devbox/);
		expect(addressNote({ address: { kind: "funnel" }, funnel: off }, "devbox")).toBe(
			"Funnel turns on for devbox when a webhook on it does."
		);
		expect(
			addressNote(
				{
					address: { kind: "funnel" },
					funnel: { state: "on", url: "https://devbox.example.ts.net" },
				},
				"devbox"
			)
		).toBe("Funnel is on, so requests reach devbox.");
		expect(
			addressNote(
				{ address: { kind: "funnel" }, funnel: { state: "needs_setup", steps: [] } },
				"devbox"
			)
		).toBeNull();
		expect(ownAddressHint(41234)).toMatch(/points at http:\/\/127\.0\.0\.1:41234\./);
		expect(ownAddressHint(null)).toMatch(/points at this device's hooks port\./);
	});

	it("hides the token, and shows only the path before the address works", () => {
		expect(hiddenHookUrl("task-1", { state: "on", url: "https://devbox.example.ts.net" })).toBe(
			"https://devbox.example.ts.net/maxhq/task-1/••••••"
		);
		expect(hiddenHookUrl("task-1", { state: "off" })).toBe("/maxhq/task-1/••••••");
	});
});

describe("needsSecret", () => {
	const hook = (hasSecret: boolean | null) =>
		task({
			trigger: {
				type: "hook",
				on: true,
				signature:
					hasSecret === null
						? null
						: {
								header: "x-hub-signature-256",
								encoding: "hex",
								prefix: "sha256=",
								has_secret: hasSecret,
							},
				rules: RULES,
				hold: false,
			},
		});

	it("asks for a secret only for a signed webhook whose device holds none", () => {
		expect(needsSecret(hook(false))).toBe(true);
		expect(needsSecret(hook(true))).toBe(false);
		expect(needsSecret(hook(null))).toBe(false);
		expect(needsSecret(task({}))).toBe(false);
	});
});

describe("the fields a prompt fills in", () => {
	it("says which are free text, and how they fill in", () => {
		expect(fieldKind({ expression: "body.title", free: true }, true)).toBe("Free text");
		expect(fieldKind({ expression: "body", free: false }, true)).toBe("Structural parts");
		expect(fieldKind({ expression: "body.number", free: false }, true)).toBe("Structural");
		expect(fieldsNote(true)).toMatch(/^Numbers, logins/);
		expect(fieldsNote(false)).toMatch(/^Every value from the request/);
	});
});
