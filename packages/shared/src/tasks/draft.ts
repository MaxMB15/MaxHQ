// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the scheduled task
// form's rules and the webhook limits. The draft is the task
// editor's form; it turns into a `Task` and back. MaxHQ saves the whole task,
// so there is no "only send changed filters" step, the target is an agent
// or a new agent's spec instead of a project and model, GitHub events join
// the triggers and take the same rules, and the daily cap and the free text
// a prompt may hold are new.

import { t } from "../i18n";

import {
	type AgentId,
	type Filter,
	type FilterTest,
	type HookRules,
	STRUCTURAL_KEYS,
	STRUCTURAL_SUFFIXES,
	type Task,
	type TaskId,
	type TaskTarget,
	type Trigger,
	type Weekday,
} from "../bindings";
import { type Choices, emptyChoices, planStart, resolve } from "../new-agent/spec";
import type { ViewState } from "../view/replica";

/** The longest "Skip requests older than", a day in minutes. */
export const MAX_AGE_MINUTES = 24 * 60;
/** The most filters a task takes. */
export const MAX_FILTERS = 16;
/** A coalesce window's shortest and longest, in seconds, and the default. */
export const COALESCE_SECS = { min: 10, max: 3600, default: 120 } as const;
/**
 * How long a window that restarts waits at most, in seconds, when the task
 * doesn't say, and the most it may say.
 */
export const MAX_WAIT_SECS = { default: 2 * 60 * 60, max: 24 * 60 * 60 } as const;

/** The prompt a new webhook task starts with. */
export const DEFAULT_HOOK_PROMPT = "Handle this webhook:\n{{body}}";
/** The prompt a new GitHub events task starts with. */
export const DEFAULT_GITHUB_PROMPT = "Handle this GitHub event:\n{{body}}";

/** GitHub's signature, which most senders copy. */
export const SIGNATURE_DEFAULTS = {
	header: "x-hub-signature-256",
	encoding: "hex",
	prefix: "sha256=",
} as const;

/** The days, Monday first, as the toggles show them. */
export const WEEKDAYS: readonly { readonly day: Weekday; readonly label: string }[] = [
	{
		day: "monday",
		get label() {
			return t("Mon");
		},
	},
	{
		day: "tuesday",
		get label() {
			return t("Tue");
		},
	},
	{
		day: "wednesday",
		get label() {
			return t("Wed");
		},
	},
	{
		day: "thursday",
		get label() {
			return t("Thu");
		},
	},
	{
		day: "friday",
		get label() {
			return t("Fri");
		},
	},
	{
		day: "saturday",
		get label() {
			return t("Sat");
		},
	},
	{
		day: "sunday",
		get label() {
			return t("Sun");
		},
	},
];

const WORKDAYS: readonly Weekday[] = ["monday", "tuesday", "wednesday", "thursday", "friday"];

/** How a filter tests its value. */
export type FilterOp = FilterTest["op"];

/** The tests a filter row offers, in the words the row shows. */
export const FILTER_OPS: readonly { readonly op: FilterOp; readonly label: string }[] = [
	{
		op: "equals",
		get label() {
			return t("equals");
		},
	},
	{
		op: "not_equals",
		get label() {
			return t("does not equal");
		},
	},
	{
		op: "in",
		get label() {
			return t("is one of");
		},
	},
	{
		op: "starts_with",
		get label() {
			return t("starts with");
		},
	},
	{
		op: "exists",
		get label() {
			return t("exists");
		},
	},
];

/**
 * One editable filter row. `in` takes a comma-separated list, which is
 * lossless because list values can't contain commas; `exists` ignores the
 * value.
 */
export type FilterDraft = {
	/** Stable row identity for the form; never saved. */
	readonly id: string;
	readonly path: string;
	readonly op: FilterOp;
	readonly value: string;
};

let lastFilterId = 0;
const nextFilterId = () => `filter-${++lastFilterId}`;

/** A blank filter row. */
export const emptyFilter = (): FilterDraft => ({
	id: nextFilterId(),
	path: "",
	op: "equals",
	value: "",
});

/** The rows for a task's filters. */
export const filterDrafts = (filters: readonly Filter[]): FilterDraft[] =>
	filters.map(({ path, test }) => ({
		id: nextFilterId(),
		path,
		op: test.op,
		value: test.op === "exists" ? "" : test.op === "in" ? test.values.join(", ") : test.value,
	}));

const FILTER_PATH = /^(body|headers|query)(\.|$)/;

/** The filters the rows describe, or why one of them can't be saved. */
export const parseFilters = (
	drafts: readonly FilterDraft[]
): { readonly filters: Filter[] } | { readonly error: string } => {
	if (drafts.length > MAX_FILTERS)
		return { error: t("Use at most {{MAX_FILTERS}} filters.", { MAX_FILTERS: MAX_FILTERS }) };
	const filters: Filter[] = [];
	for (const [index, draft] of drafts.entries()) {
		const path = draft.path.trim();
		const values = draft.value
			.split(",")
			.map((item) => item.trim())
			.filter((item) => item !== "");
		const test: FilterTest =
			draft.op === "exists"
				? { op: "exists" }
				: draft.op === "in"
					? { op: "in", values }
					: { op: draft.op, value: draft.value };
		const valid =
			FILTER_PATH.test(path) &&
			(test.op === "exists" ||
				(test.op === "in" ? test.values.length > 0 : test.value !== ""));
		if (!valid) {
			const needs =
				draft.op === "exists"
					? "."
					: draft.op === "in"
						? t(", and at least one value.")
						: t(", and a value.");
			return {
				error: t(
					"Filter {{index___1}} needs a path under body, headers or query{{needs}}",
					{ index___1: index + 1, needs: needs }
				),
			};
		}
		filters.push({ path, test });
	}
	return { filters };
};

/**
 * Whether a replay or coalesce key is a template `maxd` can fill: at least
 * one placeholder, unless `constant` allows plain text, and no stray
 * braces. A coalesce key without one, such as `all`, is one window for
 * every request.
 */
export const isKeyTemplate = (template: string, constant = false): boolean => {
	const placeholders = /\{\{\s*(?:(?:body|headers|query)(?:\.[^{}.\s]+)*|request)\s*\}\}/g;
	const matches = [...template.matchAll(placeholders)];
	const remainder = template.replace(placeholders, "");
	return (
		(constant || matches.length > 0) && !remainder.includes("{{") && !remainder.includes("}}")
	);
};

/** A placeholder a prompt uses, and whether it names free text. */
export type PromptField = {
	readonly expression: string;
	/**
	 * Whether whoever sent the request wrote it, such as a title or a
	 * comment, so it fills in only when the task allows it.
	 */
	readonly free: boolean;
};

const PLACEHOLDER = /\{\{([^{}]*)\}\}/g;

/** The key an expression ends on: its last segment that isn't a list index, without `[]`. */
const lastKey = (expression: string): string =>
	expression
		.split(".")
		.reverse()
		.map((segment) => segment.replace(/(\[\])+$/, ""))
		.find((segment) => !/^\d+$/.test(segment)) ?? "";

const structuralKey = (key: string): boolean =>
	(STRUCTURAL_KEYS as readonly string[]).includes(key) ||
	STRUCTURAL_SUFFIXES.some((end) => key.endsWith(end));

/**
 * Whether `expression` names free text, as `maxd` decides: all of any
 * webhook's request but a GitHub event's, and of that the whole request,
 * all its headers or query, and any of the body but its structural keys.
 * A GitHub event's whole body fills in its structural parts.
 */
export const isFreeText = (expression: string, github: boolean): boolean => {
	if (!github) return true;
	const dot = expression.indexOf(".");
	if (dot < 0) return expression !== "body";
	const head = expression.slice(0, dot);
	if (head === "headers" || head === "query") return false;
	if (head === "body") return !structuralKey(lastKey(expression));
	return true;
};

/** The placeholders `prompt` uses, each once, in the order they come. */
export const promptFields = (prompt: string, github: boolean): PromptField[] => {
	const seen = new Set<string>();
	for (const match of prompt.matchAll(PLACEHOLDER)) {
		const expression = (match[1] ?? "").trim();
		if (expression) seen.add(expression);
	}
	return [...seen].map((expression) => ({ expression, free: isFreeText(expression, github) }));
};

/** Blank is "no limit", null; undefined means the text isn't a valid number. */
const parseLimit = (text: string, min: number, max: number): number | null | undefined => {
	const value = text.trim();
	if (value === "") return null;
	const number = Number(value);
	return Number.isInteger(number) && number >= min && number <= max ? number : undefined;
};

/** What makes the draft's task run. */
export type TriggerKind = Trigger["type"];

/** The trigger picker's choices, in its order. */
export const TRIGGER_KINDS: readonly { readonly kind: TriggerKind; readonly label: string }[] = [
	{
		kind: "every",
		get label() {
			return t("Every");
		},
	},
	{
		kind: "at",
		get label() {
			return t("At");
		},
	},
	{
		kind: "hook",
		get label() {
			return t("Webhook");
		},
	},
	{
		kind: "github_events",
		get label() {
			return t("GitHub events");
		},
	},
];

/** The editor's form. Numbers stay text until the save, so a field can be blank. */
export type TaskDraft = {
	/** The task being edited, or null for a new one. */
	readonly saved: Task | null;
	readonly name: string;
	readonly prompt: string;
	readonly kind: TriggerKind;
	readonly minutes: string;
	/** "HH:MM". */
	readonly time: string;
	/** None and all seven both mean every day. */
	readonly days: ReadonlySet<Weekday>;
	readonly zone: string;
	readonly signed: boolean;
	readonly header: string;
	readonly encoding: "hex" | "base64";
	readonly prefix: string;
	/** Write-only: empty keeps the secret the device holds. */
	readonly secret: string;
	/** Whether the hook holds requests while its target's device is offline. */
	readonly hold: boolean;
	readonly filters: readonly FilterDraft[];
	readonly replayKey: string;
	readonly coalesceKey: string;
	readonly coalesceSecs: string;
	/** Whether each request restarts the coalesce window. */
	readonly coalesceRestart: boolean;
	/** Blank waits the default longest. */
	readonly coalesceMaxSecs: string;
	readonly maxAge: string;
	/** Empty means no cap. */
	readonly dailyRuns: string;
	/**
	 * The free text fields the prompt may hold. Null, from a task made
	 * before the list, fills in every field framed on a webhook and none on
	 * GitHub events.
	 */
	readonly fields: readonly string[] | null;
	/** Whether GitHub events run the task only for its repo's people. */
	readonly repoPeople: boolean;
	readonly repo: string;
	readonly events: ReadonlySet<string>;
	readonly target: "agent" | "new";
	/** The agent an "agent" target reaches. */
	readonly agent: AgentId | null;
	/** The new agent's device, login and place, as the new agent form picks them. */
	readonly choices: Choices;
};

/** The author associations a GitHub events task with the rule on runs for. */
const REPO_PEOPLE = ["OWNER", "MEMBER", "COLLABORATOR"];

/** The zone this device's clock is in, such as `Europe/Amsterdam`. */
export const localZone = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone;

/** A blank draft, with the defaults: every 15 minutes, or 09:00 on workdays. */
export const emptyDraft = (zone: string = localZone()): TaskDraft => ({
	saved: null,
	name: "",
	prompt: "",
	kind: "every",
	minutes: "15",
	time: "09:00",
	days: new Set(WORKDAYS),
	zone,
	signed: false,
	...SIGNATURE_DEFAULTS,
	secret: "",
	hold: false,
	filters: [],
	replayKey: "",
	coalesceKey: "",
	coalesceSecs: "",
	coalesceRestart: false,
	coalesceMaxSecs: "",
	maxAge: "",
	dailyRuns: "",
	fields: [],
	repoPeople: true,
	repo: "",
	events: new Set(),
	target: "new",
	agent: null,
	choices: emptyChoices(),
});

/**
 * The prompt to show when you switch triggers: the template default for a
 * webhook or GitHub events while the prompt is empty or still another
 * default.
 */
export const promptFor = (kind: TriggerKind, prompt: string): string => {
	if (prompt.trim() !== "" && prompt !== DEFAULT_HOOK_PROMPT && prompt !== DEFAULT_GITHUB_PROMPT)
		return prompt;
	if (kind === "hook") return DEFAULT_HOOK_PROMPT;
	if (kind === "github_events") return DEFAULT_GITHUB_PROMPT;
	return "";
};

const pad = (n: number) => String(n).padStart(2, "0");

const rulesOf = (trigger: Trigger): HookRules | null =>
	trigger.type === "hook" || trigger.type === "github_events" ? trigger.rules : null;

/** The draft for editing a saved task. */
export const taskToDraft = (view: ViewState, task: Task, zone: string = localZone()): TaskDraft => {
	const { trigger, target } = task;
	const rules = rulesOf(trigger);
	const signature = trigger.type === "hook" ? trigger.signature : null;
	const spec = target.type === "new" ? target.spec : null;
	const workspace =
		spec?.place.type === "workspace"
			? view.tables.workspace.get(spec.place.workspace)
			: undefined;
	return {
		...emptyDraft(zone),
		saved: task,
		name: task.name,
		prompt: task.prompt,
		kind: trigger.type,
		minutes: trigger.type === "every" ? String(trigger.minutes) : "15",
		time:
			trigger.type === "at"
				? `${pad(trigger.time.hour)}:${pad(trigger.time.minute)}`
				: "09:00",
		days:
			trigger.type === "at"
				? new Set(trigger.days.length > 0 ? trigger.days : WEEKDAYS.map((w) => w.day))
				: new Set(WORKDAYS),
		zone: trigger.type === "at" ? trigger.zone : zone,
		...(signature
			? {
					signed: true,
					header: signature.header,
					encoding: signature.encoding,
					prefix: signature.prefix,
				}
			: { signed: false, ...SIGNATURE_DEFAULTS }),
		hold: trigger.type === "hook" && trigger.hold,
		filters: filterDrafts(rules?.filters ?? []),
		replayKey: rules?.replay_key ?? "",
		coalesceKey: rules?.coalesce_key ?? "",
		coalesceSecs: rules?.coalesce_secs != null ? String(rules.coalesce_secs) : "",
		coalesceRestart: rules?.coalesce_restart ?? false,
		coalesceMaxSecs: rules?.coalesce_max_secs != null ? String(rules.coalesce_max_secs) : "",
		maxAge: rules?.max_age_minutes != null ? String(rules.max_age_minutes) : "",
		dailyRuns: rules?.daily_runs != null ? String(rules.daily_runs) : "",
		fields: rules?.fields ?? null,
		// A saved GitHub task keeps what it has, so one from before the rule
		// stays open to anyone. Any other task becoming one starts with it on.
		repoPeople: trigger.type === "github_events" ? (rules?.authors ?? []).length > 0 : true,
		repo: trigger.type === "github_events" ? trigger.repo : "",
		events: new Set(trigger.type === "github_events" ? trigger.events : []),
		target: target.type,
		agent: target.type === "agent" ? target.agent : null,
		choices: spec
			? {
					...emptyChoices(workspace?.project ?? null),
					place: spec.place.type === "folder" ? "folder" : "project",
					workspace: workspace?.id ?? null,
					device: target.device,
					path: spec.place.type === "folder" ? spec.place.path : "",
					account: spec.account,
					model: spec.model,
					mode: spec.permission,
				}
			: emptyChoices(),
	};
};

/** Why the webhook or GitHub events rules can't be saved, if they can't. */
export const rulesErrors = (draft: TaskDraft): string[] => {
	if (draft.kind !== "hook" && draft.kind !== "github_events") return [];
	const errors: string[] = [];
	if (parseLimit(draft.maxAge, 1, MAX_AGE_MINUTES) === undefined)
		errors.push(
			t(
				"Skip requests older than takes whole minutes from 1 to {{MAX_AGE_MINUTES}}, or blank.",
				{ MAX_AGE_MINUTES: MAX_AGE_MINUTES }
			)
		);
	const replay = draft.replayKey.trim();
	if (replay && !isKeyTemplate(replay))
		errors.push(t("Replay key needs a placeholder such as {{body.id}}."));
	const coalesce = draft.coalesceKey.trim();
	if (coalesce && !isKeyTemplate(coalesce, true))
		errors.push(t("Coalesce key needs whole placeholders such as {{body.id}}."));
	if (parseLimit(draft.coalesceSecs, COALESCE_SECS.min, COALESCE_SECS.max) === undefined)
		errors.push(
			t(
				"Coalesce window takes whole seconds from {{COALESCE_SECS_min}} to {{COALESCE_SECS_max}}.",
				{ COALESCE_SECS_min: COALESCE_SECS.min, COALESCE_SECS_max: COALESCE_SECS.max }
			)
		);
	else if (coalesce && draft.coalesceRestart && maxWait(draft) === undefined)
		errors.push(
			t(
				"Longest wait takes whole seconds from {{windowSecs_draft}} to {{MAX_WAIT_SECS_max}}, or blank for {{MAX_WAIT_SECS_default___3600}} hours.",
				{
					windowSecs_draft: windowSecs(draft),
					MAX_WAIT_SECS_max: MAX_WAIT_SECS.max,
					MAX_WAIT_SECS_default___3600: MAX_WAIT_SECS.default / 3600,
				}
			)
		);
	if (parseLimit(draft.dailyRuns, 1, Number.MAX_SAFE_INTEGER) === undefined)
		errors.push(t("Runs a day takes a whole number above 0, or blank for no cap."));
	const filters = parseFilters(draft.filters);
	if ("error" in filters) errors.push(filters.error);
	if (draft.kind === "hook" && draft.signed && draft.header.trim() === "")
		errors.push(t("Name the header the signature comes in."));
	return errors;
};

/** The coalesce window the draft gives, or the default one. */
const windowSecs = (draft: TaskDraft): number =>
	parseLimit(draft.coalesceSecs, COALESCE_SECS.min, COALESCE_SECS.max) ?? COALESCE_SECS.default;

/** The draft's longest wait: null for the default, undefined when the text isn't one. */
const maxWait = (draft: TaskDraft): number | null | undefined =>
	parseLimit(draft.coalesceMaxSecs, windowSecs(draft), MAX_WAIT_SECS.max);

/** The free text fields the draft allows that its prompt still uses. */
const allowedFields = (draft: TaskDraft): string[] | null => {
	const allowed = draft.fields;
	if (allowed === null) return null;
	return promptFields(draft.prompt, draft.kind === "github_events")
		.filter((field) => field.free && allowed.includes(field.expression))
		.map((field) => field.expression);
};

const rulesFrom = (draft: TaskDraft): HookRules => {
	const filters = parseFilters(draft.filters);
	const text = (value: string) => value.trim() || null;
	const coalesce = text(draft.coalesceKey);
	const restart = coalesce !== null && draft.coalesceRestart;
	const fields = allowedFields(draft);
	return {
		filters: "filters" in filters ? filters.filters : [],
		replay_key: text(draft.replayKey),
		coalesce_key: coalesce,
		// A coalesce key needs a window, so it takes the default.
		coalesce_secs:
			parseLimit(draft.coalesceSecs, COALESCE_SECS.min, COALESCE_SECS.max) ??
			(coalesce ? COALESCE_SECS.default : null),
		coalesce_restart: restart,
		coalesce_max_secs: restart ? (maxWait(draft) ?? null) : null,
		max_age_minutes: parseLimit(draft.maxAge, 1, MAX_AGE_MINUTES) ?? null,
		daily_runs: parseLimit(draft.dailyRuns, 1, Number.MAX_SAFE_INTEGER) ?? null,
		// A task from before the list keeps having none.
		...(fields === null ? {} : { fields }),
		...(draft.kind === "github_events" && draft.repoPeople ? { authors: REPO_PEOPLE } : {}),
	};
};

const triggerFrom = (draft: TaskDraft): Trigger | string => {
	switch (draft.kind) {
		case "every": {
			const minutes = parseLimit(draft.minutes, 1, Number.MAX_SAFE_INTEGER);
			return minutes ? { type: "every", minutes } : t("Run it every 1 minute or more.");
		}
		case "at": {
			const match = /^(\d{1,2}):(\d{2})$/.exec(draft.time.trim());
			const hour = Number(match?.[1]);
			const minute = Number(match?.[2]);
			if (!match || hour > 23 || minute > 59) return t("Pick a time of day.");
			const everyDay = draft.days.size === 0 || draft.days.size === WEEKDAYS.length;
			return {
				type: "at",
				time: { hour, minute },
				days: everyDay ? [] : WEEKDAYS.map((w) => w.day).filter((d) => draft.days.has(d)),
				zone: draft.zone,
			};
		}
		case "hook": {
			const kept = draft.saved?.trigger.type === "hook" ? draft.saved.trigger : null;
			return {
				type: "hook",
				// A new hook starts off; `setHookOn` turns it on.
				on: kept?.on ?? false,
				signature: draft.signed
					? {
							header: draft.header.trim(),
							encoding: draft.encoding,
							prefix: draft.prefix,
							has_secret:
								kept?.signature?.has_secret === true || draft.secret.trim() !== "",
						}
					: null,
				rules: rulesFrom(draft),
				hold: draft.hold,
			};
		}
		case "github_events":
			if (!draft.repo) return t("Pick a repository.");
			if (draft.events.size === 0) return t("Pick at least one event.");
			return {
				type: "github_events",
				repo: draft.repo,
				events: [...draft.events].sort(),
				rules: rulesFrom(draft),
			};
	}
};

/** Where a draft's runs go, or why it can't be saved yet. */
export const targetFrom = (view: ViewState, draft: TaskDraft): TaskTarget | string => {
	if (draft.target === "agent") {
		const agent = draft.agent ? view.tables.agent.get(draft.agent) : undefined;
		const device = agent ? view.tables.workspace.get(agent.workspace)?.device : undefined;
		if (!agent || !device) return t("Pick the agent it sends the prompt to.");
		return { type: "agent", device, agent: agent.id };
	}
	const plan = planStart(view, draft.choices);
	const device = resolve(view, draft.choices).device;
	if (!plan.ok || !device) return plan.ok ? t("Pick where the agent runs.") : plan.reason;
	return { type: "new", device, spec: plan.spec };
};

/** The permission a new task is sent with, since `maxd` sets its own. */
export const NO_PERMISSION = "00000000-0000-0000-0000-000000000000";

/** The task a draft saves, or why it can't be saved. */
export type DraftResult =
	{ readonly ok: true; readonly task: Task } | { readonly ok: false; readonly reason: string };

/**
 * The task the draft describes. A new task takes `id`, a nil permission
 * that `maxd` replaces with one scoped to its target, and you as its
 * maker; an edit keeps what `maxd` set.
 */
export const taskFromDraft = (view: ViewState, draft: TaskDraft, id: TaskId): DraftResult => {
	const name = draft.name.trim();
	if (!name) return { ok: false, reason: t("Give it a name.") };
	if (!draft.prompt.trim()) return { ok: false, reason: t("Write the prompt it sends.") };
	const errors = rulesErrors(draft);
	if (errors[0]) return { ok: false, reason: errors[0] };
	const trigger = triggerFrom(draft);
	if (typeof trigger === "string") return { ok: false, reason: trigger };
	const target = targetFrom(view, draft);
	if (typeof target === "string") return { ok: false, reason: target };
	const saved = draft.saved;
	return {
		ok: true,
		task: {
			id: saved?.id ?? id,
			name,
			prompt: draft.prompt,
			trigger,
			target,
			enabled: saved?.enabled ?? true,
			permission: saved?.permission ?? NO_PERMISSION,
			made_by: saved?.made_by ?? { type: "user" },
			next_run: saved?.next_run ?? null,
			last_run: saved?.last_run ?? null,
		},
	};
};
