// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Task, Weekday } from "../bindings";
import { mockTasks } from "../data/mock-tasks";
import { DEVBOX, DEVBOX_CODEX, MAC, mockEntities } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	allowField,
	DEFAULT_GITHUB_PROMPT,
	DEFAULT_HOOK_PROMPT,
	emptyDraft,
	emptyFilter,
	filterDrafts,
	isFreeText,
	isKeyTemplate,
	NO_PERMISSION,
	onClock,
	parseFilters,
	promptFields,
	promptFor,
	rulesErrors,
	SIGNATURE_DEFAULTS,
	type TaskDraft,
	taskFromDraft,
	taskToDraft,
	takesRequests,
	toggled,
	withFilter,
} from "./draft";

const sample = mockTasks({ now: () => NOW });
const view = applySnapshot({ seq: 1, entities: [...mockEntities(NOW), ...sample.entities()] });
const saved = (id: string): Task => {
	const task = sample.entities().flatMap((e) => (e.entity === "device_tasks" ? e.tasks : []));
	return task.find((t) => t.id === id)!;
};

const draft = (over: Partial<TaskDraft>): TaskDraft => ({
	...emptyDraft("Europe/Amsterdam"),
	name: "Nightly",
	prompt: "Run the tests",
	target: "agent",
	agent: "agent-release",
	...over,
});

const built = (over: Partial<TaskDraft>) => {
	const result = taskFromDraft(view, draft(over), "task-new");
	if (!result.ok) throw new Error(result.reason);
	return result.task;
};

describe("a new task", () => {
	it("starts every 15 minutes, or at 09:00 on workdays, in this zone", () => {
		const fresh = emptyDraft("Europe/Amsterdam");
		expect(fresh.minutes).toBe("15");
		expect(fresh.time).toBe("09:00");
		expect([...fresh.days]).toEqual(["monday", "tuesday", "wednesday", "thursday", "friday"]);
		expect(fresh.zone).toBe("Europe/Amsterdam");
		expect(fresh).toMatchObject(SIGNATURE_DEFAULTS);
	});

	it("is yours, on, and leaves its permission for maxd to scope", () => {
		expect(built({ kind: "every", minutes: "30" })).toMatchObject({
			id: "task-new",
			name: "Nightly",
			trigger: { type: "every", minutes: 30 },
			target: { type: "agent", device: MAC, agent: "agent-release" },
			enabled: true,
			permission: NO_PERMISSION,
			made_by: { type: "user" },
			next_run: null,
			last_run: null,
		});
	});

	it("saves no days for every day, and the days in order otherwise", () => {
		const all = new Set([
			"sunday",
			"monday",
			"tuesday",
			"wednesday",
			"thursday",
			"friday",
			"saturday",
		] as const);
		expect(built({ kind: "at", days: all }).trigger).toMatchObject({ days: [] });
		expect(built({ kind: "at", days: new Set() }).trigger).toMatchObject({ days: [] });
		expect(
			built({ kind: "at", time: "7:05", days: new Set(["friday", "monday"] as const) })
				.trigger
		).toEqual({
			type: "at",
			time: { hour: 7, minute: 5 },
			days: ["monday", "friday"],
			zone: "Europe/Amsterdam",
		});
	});

	it("starts a new agent from the new agent form's choices", () => {
		const task = built({
			target: "new",
			choices: { ...emptyDraft().choices, project: "proj-atlas-maps", account: DEVBOX_CODEX },
		});
		expect(task.target).toEqual({
			type: "new",
			device: DEVBOX,
			spec: expect.objectContaining({
				vendor: "codex",
				account: DEVBOX_CODEX,
				place: { type: "workspace", workspace: "ws-alpha" },
				title: null,
				first: null,
			}),
		});
	});

	it("makes a webhook that starts off, with its rules", () => {
		const task = built({
			kind: "hook",
			signed: true,
			secret: "s3cret",
			filters: [{ ...emptyFilter(), path: "body.action", op: "in", value: "opened, closed" }],
			replayKey: "{{headers.x-github-delivery}}",
			coalesceKey: "{{body.pull_request.number}}",
			maxAge: "30",
			dailyRuns: "100",
			hold: true,
		});
		expect(task.trigger).toEqual({
			type: "hook",
			on: false,
			signature: { ...SIGNATURE_DEFAULTS, has_secret: true },
			rules: {
				filters: [
					{ path: "body.action", test: { op: "in", values: ["opened", "closed"] } },
				],
				replay_key: "{{headers.x-github-delivery}}",
				coalesce_key: "{{body.pull_request.number}}",
				coalesce_secs: 120,
				coalesce_restart: false,
				coalesce_max_secs: null,
				max_age_minutes: 30,
				daily_runs: 100,
				fields: [],
			},
			hold: true,
		});
	});

	it("leaves the daily cap off when blank", () => {
		const task = built({ kind: "hook" });
		expect(task.trigger).toMatchObject({ rules: { daily_runs: null, max_age_minutes: null } });
	});

	it("waits for a quiet stretch across every request, and reads back", () => {
		const reviews = {
			kind: "github_events",
			repo: "owner/repo",
			events: new Set(["pull_request"]),
			coalesceKey: "all",
			coalesceSecs: "900",
			coalesceRestart: true,
		} as const;
		const task = built(reviews);
		expect(task.trigger).toMatchObject({
			rules: {
				coalesce_key: "all",
				coalesce_secs: 900,
				coalesce_restart: true,
				coalesce_max_secs: null,
			},
		});
		expect(taskToDraft(view, task)).toMatchObject({
			coalesceKey: "all",
			coalesceRestart: true,
			coalesceMaxSecs: "",
		});
		const capped = built({ ...reviews, coalesceMaxSecs: "3600" });
		expect(capped.trigger).toMatchObject({ rules: { coalesce_max_secs: 3600 } });
		expect(taskToDraft(view, capped).coalesceMaxSecs).toBe("3600");
		// Restart means nothing without a key.
		const keyless = built({ ...reviews, coalesceKey: "", coalesceMaxSecs: "3600" });
		expect(keyless.trigger).toMatchObject({
			rules: { coalesce_restart: false, coalesce_max_secs: null },
		});
	});

	it("watches GitHub events in order", () => {
		const task = built({
			kind: "github_events",
			repo: "Example-User/MaxHQ",
			events: new Set(["push", "issues"]),
		});
		expect(task.trigger).toMatchObject({
			type: "github_events",
			repo: "Example-User/MaxHQ",
			events: ["issues", "push"],
		});
	});
});

describe("why a draft can't be saved", () => {
	const reason = (over: Partial<TaskDraft>) => {
		const result = taskFromDraft(view, draft(over), "task-new");
		return result.ok ? null : result.reason;
	};

	it("needs a name, a prompt and a target", () => {
		expect(reason({ name: " " })).toBe("Give it a name.");
		expect(reason({ prompt: "" })).toBe("Write the prompt it sends.");
		expect(reason({ agent: null })).toBe("Pick the agent it sends the prompt to.");
		expect(
			reason({ target: "new", choices: { ...emptyDraft().choices, place: "folder" } })
		).toBe("Type the folder's full path, starting with /");
	});

	it("checks the trigger", () => {
		expect(reason({ minutes: "0" })).toBe("Run it every 1 minute or more.");
		expect(reason({ kind: "at", time: "25:00" })).toBe("Pick a time of day.");
		expect(reason({ kind: "github_events" })).toBe("Pick a repository.");
		expect(reason({ kind: "github_events", repo: "o/r" })).toBe("Pick at least one event.");
	});

	it("keeps a new GitHub task to the repo's people until you turn that off", () => {
		const github = { kind: "github_events", repo: "o/r", events: new Set(["issues"]) } as const;
		const rulesOf = (over: Partial<TaskDraft>) => {
			const { trigger } = built(over);
			if (trigger.type !== "github_events") throw new Error(trigger.type);
			return trigger.rules;
		};
		const people = ["OWNER", "MEMBER", "COLLABORATOR"];
		expect(rulesOf(github).authors).toEqual(people);
		expect(rulesOf({ ...github, repoPeople: false })).not.toHaveProperty("authors");
		expect(built({ kind: "hook" }).trigger).not.toHaveProperty("rules.authors");
		// No daily cap comes with it.
		expect(rulesOf(github).daily_runs).toBeNull();
	});

	it("leaves a saved GitHub task's rule as it was", () => {
		const saved = (authors?: string[]) => {
			const github = {
				kind: "github_events",
				repo: "o/r",
				events: new Set(["issues"]),
				repoPeople: false,
			} as const;
			const task = built(github);
			if (task.trigger.type !== "github_events") throw new Error(task.trigger.type);
			const { rules } = task.trigger;
			return taskToDraft(view, {
				...task,
				trigger: { ...task.trigger, rules: authors ? { ...rules, authors } : rules },
			});
		};
		expect(saved().repoPeople).toBe(false);
		expect(saved([]).repoPeople).toBe(false);
		expect(saved(["OWNER", "MEMBER", "COLLABORATOR"]).repoPeople).toBe(true);
		const reopened = taskFromDraft(view, saved(), "task-new");
		if (!reopened.ok) throw new Error(reopened.reason);
		expect(reopened.task.trigger).not.toHaveProperty("rules.authors");
	});

	it("checks the webhook rules", () => {
		const errors = (over: Partial<TaskDraft>) => rulesErrors(draft({ kind: "hook", ...over }));
		expect(errors({})).toEqual([]);
		expect(rulesErrors(draft({ kind: "every", maxAge: "abc" }))).toEqual([]);
		expect(errors({ maxAge: "1441" })[0]).toMatch(/1 to 1,440/);
		expect(errors({ maxAge: "1.5" })[0]).toMatch(/1 to 1,440/);
		expect(errors({ replayKey: "no placeholder" })).toEqual([
			"Replay key needs a placeholder such as {{body.id}}.",
		]);
		expect(errors({ coalesceSecs: "5" })[0]).toMatch(/10 to 3,600/);
		expect(errors({ coalesceKey: "all" })).toEqual([]);
		expect(errors({ coalesceKey: "pr-{{body.number" })).toEqual([
			"Coalesce key needs whole placeholders such as {{body.id}}.",
		]);
		const restart = { coalesceKey: "all", coalesceSecs: "900", coalesceRestart: true };
		expect(errors({ ...restart, coalesceMaxSecs: "" })).toEqual([]);
		expect(errors({ ...restart, coalesceMaxSecs: "600" })).toEqual([
			"Longest wait takes whole seconds from 900 to 86,400, or blank for 2 hours.",
		]);
		expect(errors({ ...restart, coalesceMaxSecs: "86401" })[0]).toMatch(/900 to 86,400/);
		expect(errors({ dailyRuns: "0" })[0]).toMatch(/Runs a day/);
		expect(errors({ signed: true, header: " " })).toEqual([
			"Name the header the signature comes in.",
		]);
		expect(errors({ kind: "github_events", filters: [emptyFilter()] })).toEqual([
			"Filter 1 needs a path under body, headers or query, and a value.",
		]);
	});
});

describe("key templates", () => {
	it("takes placeholders and refuses stray braces", () => {
		expect(isKeyTemplate("{{body.id}}")).toBe(true);
		expect(isKeyTemplate("pr-{{ body.pull_request.number }}-{{headers.x-event}}")).toBe(true);
		expect(isKeyTemplate("{{request}}")).toBe(true);
		expect(isKeyTemplate("plain")).toBe(false);
		expect(isKeyTemplate("{{body.id}} }}")).toBe(false);
		expect(isKeyTemplate("{{env.HOME}}")).toBe(false);
	});

	it("takes plain text for a coalesce key, which is one window for every request", () => {
		expect(isKeyTemplate("all", true)).toBe(true);
		expect(isKeyTemplate("pr-{{body.number}}", true)).toBe(true);
		expect(isKeyTemplate("all }}", true)).toBe(false);
	});
});

describe("filters", () => {
	it("round-trips each test, with `in` as a comma list", () => {
		const filters = [
			{ path: "headers.x-github-event", test: { op: "equals" as const, value: "push" } },
			{ path: "body.action", test: { op: "in" as const, values: ["a", "b"] } },
			{ path: "query.token", test: { op: "exists" as const } },
		];
		const rows = filterDrafts(filters);
		expect(rows.map((r) => r.value)).toEqual(["push", "a, b", ""]);
		expect(new Set(rows.map((r) => r.id)).size).toBe(3);
		expect(parseFilters(rows)).toEqual({ filters });
	});

	it("names the row that's wrong, and caps the count", () => {
		const good = { ...emptyFilter(), path: "body.a", value: "x" };
		expect(parseFilters([good, { ...good, path: "env.x" }])).toEqual({
			error: "Filter 2 needs a path under body, headers or query, and a value.",
		});
		expect(parseFilters([{ ...good, op: "in", value: " , " }])).toEqual({
			error: "Filter 1 needs a path under body, headers or query, and at least one value.",
		});
		expect(parseFilters([{ ...good, op: "exists", path: "" }])).toEqual({
			error: "Filter 1 needs a path under body, headers or query.",
		});
		expect(parseFilters(Array.from({ length: 17 }, () => good))).toEqual({
			error: "Use at most 16 filters.",
		});
	});
});

describe("editing a saved task", () => {
	it("keeps what maxd set, and what the form doesn't show", () => {
		const triage = saved("task-triage");
		const editing = taskToDraft(view, triage, "Europe/Amsterdam");
		expect(editing).toMatchObject({
			kind: "hook",
			signed: true,
			secret: "",
			replayKey: "{{headers.x-github-delivery}}",
			maxAge: "60",
			target: "new",
			choices: { project: "proj-atlas-maps", workspace: "ws-alpha", account: DEVBOX_CODEX },
		});
		const result = taskFromDraft(view, { ...editing, name: "Triage" }, "unused");
		expect(result).toEqual({ ok: true, task: { ...triage, name: "Triage" } });
	});

	it("reads an At task back in its own zone", () => {
		const editing = taskToDraft(view, saved("task-morning"), "UTC");
		expect(editing).toMatchObject({ kind: "at", time: "09:00", zone: "Europe/Amsterdam" });
		expect(taskFromDraft(view, editing, "unused")).toEqual({
			ok: true,
			task: saved("task-morning"),
		});
	});

	it("shows every day as all seven toggles", () => {
		const daily: Task = {
			...saved("task-morning"),
			trigger: { type: "at", time: { hour: 6, minute: 0 }, days: [], zone: "UTC" },
		};
		expect(taskToDraft(view, daily).days.size).toBe(7);
	});

	it("round-trips GitHub events and an agent target", () => {
		for (const id of ["task-reviews", "task-checklist"]) {
			expect(taskFromDraft(view, taskToDraft(view, saved(id)), "unused")).toEqual({
				ok: true,
				task: saved(id),
			});
		}
	});

	it("starts the agent fresh each run only when asked, and reads that back", () => {
		expect(built({}).target).toEqual({ type: "agent", device: MAC, agent: "agent-release" });
		const fresh = built({ fresh: true });
		expect(fresh.target).toEqual({
			type: "agent",
			device: MAC,
			agent: "agent-release",
			fresh: true,
		});
		expect(taskToDraft(view, fresh).fresh).toBe(true);
		expect(taskToDraft(view, saved("task-checklist")).fresh).toBe(false);
		// A new agent starts fresh anyway, so the switch doesn't go with it.
		const choices = {
			...emptyDraft().choices,
			project: "proj-atlas-maps",
			account: DEVBOX_CODEX,
		};
		expect(built({ fresh: true, target: "new", choices }).target).not.toHaveProperty("fresh");
	});
});

describe("the fields a prompt fills in", () => {
	it("marks free text as maxd does", () => {
		const prompt =
			"PR {{ body.pull_request.number }} by {{body.sender.login}}: {{body.pull_request.title}}\n" +
			"{{body.pull_request.html_url}} {{body.commits[].id}} {{headers.x-github-event}} " +
			"{{body}} {{request}} {{body.pull_request.title}} {{}}";
		expect(promptFields(prompt, true)).toEqual([
			{ expression: "body.pull_request.number", free: false },
			{ expression: "body.sender.login", free: false },
			{ expression: "body.pull_request.title", free: true },
			{ expression: "body.pull_request.html_url", free: false },
			{ expression: "body.commits[].id", free: false },
			{ expression: "headers.x-github-event", free: false },
			{ expression: "body", free: false },
			{ expression: "request", free: true },
		]);
		// Any other webhook's request is all free text, headers and query too.
		expect(isFreeText("body.pull_request.number", false)).toBe(true);
		expect(isFreeText("body", false)).toBe(true);
		expect(isFreeText("query.page", false)).toBe(true);
		expect(isFreeText("headers.x-github-event", false)).toBe(true);
		expect(isFreeText("query.page", true)).toBe(false);
		expect(isFreeText("headers", true)).toBe(true);
		expect(isFreeText("body.items.0", true)).toBe(true);
		expect(isFreeText("body.items.0.created_at", true)).toBe(false);
	});

	it("starts a new task with no free text, and saves only what the prompt uses", () => {
		const reviews = {
			kind: "github_events",
			repo: "owner/repo",
			events: new Set(["pull_request"]),
			prompt: "Review {{body.pull_request.html_url}}: {{body.pull_request.title}}",
		} as const;
		expect(built(reviews).trigger).toMatchObject({ rules: { fields: [] } });
		const allowed = built({
			...reviews,
			fields: ["body.pull_request.title", "body.pull_request.body"],
		});
		expect(allowed.trigger).toMatchObject({ rules: { fields: ["body.pull_request.title"] } });
		expect(taskToDraft(view, allowed).fields).toEqual(["body.pull_request.title"]);
	});

	it("keeps a webhook from before the list as it was until you pick the safe default", () => {
		const triage = saved("task-triage");
		const editing = taskToDraft(view, triage);
		expect(editing.fields).toBeNull();
		expect(taskFromDraft(view, editing, "unused")).toEqual({ ok: true, task: triage });
		const safe = taskFromDraft(view, { ...editing, fields: [] }, "unused");
		expect(safe.ok && safe.task.trigger).toMatchObject({ rules: { fields: [] } });
	});
});

describe("the prompt a trigger starts with", () => {
	it("fills a template default and keeps what you wrote", () => {
		expect(promptFor("hook", "")).toBe(DEFAULT_HOOK_PROMPT);
		expect(promptFor("github_events", DEFAULT_HOOK_PROMPT)).toBe(DEFAULT_GITHUB_PROMPT);
		expect(promptFor("every", DEFAULT_HOOK_PROMPT)).toBe("");
		expect(promptFor("hook", "Mine")).toBe("Mine");
	});
});

describe("editing the form", () => {
	it("says which triggers take requests", () => {
		expect(takesRequests("hook")).toBe(true);
		expect(takesRequests("github_events")).toBe(true);
		expect(takesRequests("every")).toBe(false);
		expect(takesRequests("at")).toBe(false);
	});

	it("turns a day or an event on and off without changing the set it had", () => {
		const days = new Set(["mon"] as const);
		expect([...toggled(days, "tue", true)]).toEqual(["mon", "tue"]);
		expect([...toggled(days, "mon", false)]).toEqual([]);
		expect([...days]).toEqual(["mon"]);
	});

	it("allows a field, from a list from before the choice too", () => {
		expect(allowField(null, "body.title", true)).toEqual(["body.title"]);
		expect(allowField(["body.title", "body.body"], "body.title", false)).toEqual(["body.body"]);
	});

	it("changes and removes a filter row", () => {
		const rows = [emptyFilter(), emptyFilter()];
		const changed = { ...rows[1]!, path: "body.action" };
		expect(withFilter(rows, 1, changed)).toEqual([rows[0], changed]);
		expect(withFilter(rows, 0, null)).toEqual([rows[1]]);
	});
});

describe("a task's time on another clock", () => {
	const JUNE = new Date("2026-06-10T12:00:00Z");
	const at = (time: string, ...days: Weekday[]) => ({ time, days: new Set(days) });
	const ALL: Weekday[] = [
		"monday",
		"tuesday",
		"wednesday",
		"thursday",
		"friday",
		"saturday",
		"sunday",
	];

	it("moves the time ahead for a zone ahead of the task's", () => {
		expect(
			onClock(
				at("09:00", "monday", "wednesday"),
				"America/New_York",
				"Europe/Amsterdam",
				JUNE
			)
		).toEqual(at("15:00", "monday", "wednesday"));
	});

	it("moves the time back for a zone behind the task's", () => {
		expect(
			onClock(
				at("09:00", "monday", "wednesday"),
				"Europe/Amsterdam",
				"America/New_York",
				JUNE
			)
		).toEqual(at("03:00", "monday", "wednesday"));
	});

	it("moves the days on when the time crosses midnight forward", () => {
		expect(
			onClock(at("23:00", "monday", "sunday"), "Europe/Amsterdam", "Asia/Tokyo", JUNE)
		).toEqual(at("06:00", "tuesday", "monday"));
	});

	it("moves the days back when the time crosses midnight backward", () => {
		expect(
			onClock(at("06:00", "tuesday", "monday"), "Asia/Tokyo", "Europe/Amsterdam", JUNE)
		).toEqual(at("23:00", "monday", "sunday"));
		expect(
			onClock(at("01:00", "monday"), "Europe/Amsterdam", "America/New_York", JUNE)
		).toEqual(at("19:00", "sunday"));
	});

	it("keeps every day as every day", () => {
		expect(onClock(at("23:00"), "Europe/Amsterdam", "Asia/Tokyo", JUNE)).toEqual(at("06:00"));
		expect(onClock(at("23:00", ...ALL), "Europe/Amsterdam", "Asia/Tokyo", JUNE)).toEqual(
			at("06:00", ...ALL)
		);
	});

	it("reads the gap on the day, when only one zone has moved to summer time", () => {
		// New York moves on 2026-03-08, Amsterdam on 2026-03-29.
		const between = new Date("2026-03-20T12:00:00Z");
		expect(onClock(at("09:00"), "Europe/Amsterdam", "America/New_York", between)).toEqual(
			at("04:00")
		);
		expect(
			onClock(
				at("09:00"),
				"Europe/Amsterdam",
				"America/New_York",
				new Date("2026-04-01T12:00:00Z")
			)
		).toEqual(at("03:00"));
		// Amsterdam moves back on 2026-10-25, New York on 2026-11-01.
		expect(
			onClock(
				at("09:00"),
				"Europe/Amsterdam",
				"America/New_York",
				new Date("2026-10-28T12:00:00Z")
			)
		).toEqual(at("04:00"));
		expect(onClock(at("04:00"), "America/New_York", "Europe/Amsterdam", between)).toEqual(
			at("09:00")
		);
	});

	it("takes half hours and gaps of more than a day", () => {
		expect(onClock(at("00:15", "monday"), "Asia/Kolkata", "Europe/Amsterdam", JUNE)).toEqual(
			at("20:45", "sunday")
		);
		// Kiritimati is 14 hours ahead of UTC and Pago Pago 11 behind.
		expect(
			onClock(at("00:30", "monday"), "Pacific/Kiritimati", "Pacific/Pago_Pago", JUNE)
		).toEqual(at("23:30", "saturday"));
		expect(
			onClock(at("23:30", "saturday"), "Pacific/Pago_Pago", "Pacific/Kiritimati", JUNE)
		).toEqual(at("00:30", "monday"));
	});

	it("comes back as it was", () => {
		const zones = [
			"Europe/Amsterdam",
			"Asia/Kolkata",
			"America/Los_Angeles",
			"Pacific/Kiritimati",
		];
		for (const from of zones)
			for (const to of zones)
				for (const time of ["00:00", "06:15", "12:30", "23:59"]) {
					const kept = at(time, "monday", "friday", "sunday");
					expect(onClock(onClock(kept, from, to, JUNE), to, from, JUNE)).toEqual(kept);
				}
	});

	it("leaves the same clock, a zone it doesn't know and a time that isn't one alone", () => {
		const kept = at("09:00", "monday");
		expect(onClock(kept, "Europe/Amsterdam", "Europe/Amsterdam", JUNE)).toEqual(kept);
		expect(
			onClock(
				kept,
				"Mars/Olympus_Mons",
				Intl.DateTimeFormat().resolvedOptions().timeZone,
				JUNE
			)
		).toEqual(kept);
		const blank = at("", "monday");
		expect(onClock(blank, "Europe/Amsterdam", "Asia/Tokyo", JUNE)).toEqual(blank);
	});

	it("gives back just the time and days", () => {
		const editing = draft({ kind: "at" });
		expect(Object.keys(onClock(editing, editing.zone, editing.zone, JUNE)).sort()).toEqual([
			"days",
			"time",
		]);
	});

	it("saves an untouched task as it was, after showing it on another clock", () => {
		const editing = taskToDraft(view, saved("task-morning"), "Asia/Tokyo");
		const shown = onClock(editing, editing.zone, "Asia/Tokyo", JUNE);
		const back = onClock(shown, "Asia/Tokyo", editing.zone, JUNE);
		expect(taskFromDraft(view, { ...editing, ...back }, "unused")).toEqual({
			ok: true,
			task: saved("task-morning"),
		});
	});
});
