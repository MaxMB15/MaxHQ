// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Task } from "../bindings";
import { mockTasks } from "../data/mock-tasks";
import { DEVBOX, DEVBOX_CODEX, MAC, mockEntities } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	DEFAULT_GITHUB_PROMPT,
	DEFAULT_HOOK_PROMPT,
	emptyDraft,
	emptyFilter,
	filterDrafts,
	isFreeText,
	isKeyTemplate,
	NO_PERMISSION,
	parseFilters,
	promptFields,
	promptFor,
	rulesErrors,
	SIGNATURE_DEFAULTS,
	type TaskDraft,
	taskFromDraft,
	taskToDraft,
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
