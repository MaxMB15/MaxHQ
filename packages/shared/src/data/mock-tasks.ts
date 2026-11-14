// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The scheduled task calls of the sample data and of tests. The samples
// hold one task of each trigger on the Mac and devbox, with runs and
// webhook requests behind them, and every Funnel state: devbox reaches the
// internet, the Mac needs setting up, and a device with no webhook on is
// off. Both start on Funnel, and either can pick another address. A
// GitHub events task hears of events through a webhook unless its device
// is reached only on itself. Changes live in memory, and each device's
// tasks go out as its `device_tasks` entity the way `maxd` publishes them.

import type {
	DeviceId,
	DeviceTasks,
	Entity,
	FunnelState,
	HookAddress,
	HookRules,
	Run,
	Task,
	TaskDelivery,
	TaskId,
	TaskWebhook,
	Timestamp,
	Trigger,
} from "../bindings";
import { CLAUDE, DEVBOX, DEVBOX_CLAUDE, DEVBOX_CODEX, MAC, MHQ_WORKSPACE } from "./mock-view";
import type { ActionResult, DataSource } from "./source";
import { tasksNotBuilt } from "./tasks";

/** The scheduled task calls of a data source. */
export type TaskCalls = Pick<
	DataSource,
	| "listTasks"
	| "saveTask"
	| "setTaskEnabled"
	| "deleteTask"
	| "runTaskNow"
	| "setHookOn"
	| "rotateHookToken"
	| "setHookSecret"
	| "setHookAddress"
	| "listRuns"
	| "listDeliveries"
	| "funnelState"
>;

/** The scheduled task calls with no tasks behind them, refused the way an older `maxd` does. */
export const noTasks: TaskCalls = {
	listTasks: async () => ({ status: "ok", data: [] }),
	saveTask: tasksNotBuilt,
	setTaskEnabled: tasksNotBuilt,
	deleteTask: tasksNotBuilt,
	runTaskNow: tasksNotBuilt,
	setHookOn: tasksNotBuilt,
	rotateHookToken: tasksNotBuilt,
	setHookSecret: tasksNotBuilt,
	setHookAddress: tasksNotBuilt,
	listRuns: async () => ({ status: "ok", data: [] }),
	listDeliveries: async () => ({ status: "ok", data: [] }),
	funnelState: async () => ({ status: "ok", data: { state: "off" } }),
};

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const DEVICES: readonly DeviceId[] = [MAC, DEVBOX];

/** Each sample device's address over Funnel. devbox's reaches the internet. */
/** The port each device's hook listener takes requests on. */
const PORT = 41234;

const FUNNEL_HOST: Record<DeviceId, string> = {
	[MAC]: "https://macbook-pro.tail1234.ts.net",
	[DEVBOX]: "https://devbox.tail1234.ts.net",
};

/** What the Mac needs before its webhooks reach the internet. */
const MAC_SETUP: FunnelState = {
	state: "needs_setup",
	steps: [
		{ reason: "HTTPS certificates are off for this tailnet.", command: null },
		{
			reason: "maxd can't change Tailscale's settings as your user.",
			command: "sudo tailscale set --operator=$USER",
		},
	],
};

/** A webhook's sample rules, with nothing set. */
export const NO_RULES: HookRules = {
	filters: [],
	replay_key: null,
	coalesce_key: null,
	coalesce_secs: null,
	coalesce_restart: false,
	coalesce_max_secs: null,
	max_age_minutes: null,
	daily_runs: null,
};

/** The next time an `At` trigger comes round after `after`, in this machine's zone. */
const nextAt = (trigger: Extract<Trigger, { type: "at" }>, after: Timestamp): Timestamp => {
	const names = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];
	const day = new Date(after);
	day.setHours(trigger.time.hour, trigger.time.minute, 0, 0);
	for (let i = 0; i < 8; i += 1) {
		const runs =
			trigger.days.length === 0 || trigger.days.some((d) => d === names[day.getDay()]);
		if (runs && day.getTime() > after) return day.getTime();
		day.setDate(day.getDate() + 1);
	}
	return day.getTime();
};

/** When a task's timer fires next, or null when it has none or is paused. */
const nextRun = (task: Task, now: Timestamp): Timestamp | null => {
	if (!task.enabled) return null;
	if (task.trigger.type === "every") return now + task.trigger.minutes * MIN;
	if (task.trigger.type === "at") return nextAt(task.trigger, now);
	return null;
};

const sampleTasks = (now: Timestamp): { home: DeviceId; task: Task }[] => {
	const at: Extract<Trigger, { type: "at" }> = {
		type: "at",
		time: { hour: 9, minute: 0 },
		days: ["monday", "tuesday", "wednesday", "thursday", "friday"],
		zone: "Europe/Amsterdam",
	};
	const morning: Task = {
		id: "task-morning",
		name: "Morning summary",
		prompt: "Summarize what changed on main since yesterday and what's still open.",
		trigger: at,
		target: {
			type: "new",
			device: MAC,
			spec: {
				vendor: "claude",
				account: CLAUDE,
				place: { type: "workspace", workspace: "ws-mac-maxhq" },
				model: null,
				permission: "plan",
				title: null,
				first: null,
				branch: null,
			},
		},
		enabled: true,
		permission: "perm-task-morning",
		made_by: { type: "user" },
		next_run: nextAt(at, now),
		last_run: { at: now - 20 * HOUR, outcome: "done" },
	};
	return [
		{ home: MAC, task: morning },
		{
			home: MAC,
			task: {
				id: "task-checklist",
				name: "Check the release checklist",
				prompt: "Go through the checklist again and tick what's done.",
				trigger: { type: "every", minutes: 30 },
				target: { type: "agent", device: MAC, agent: "agent-release" },
				enabled: true,
				permission: "perm-task-checklist",
				made_by: { type: "user" },
				next_run: now + 12 * MIN,
				last_run: { at: now - 18 * MIN, outcome: "failed" },
			},
		},
		{
			home: MAC,
			task: {
				id: "task-deploy",
				name: "Deploy preview",
				prompt: "Deploy {{body.ref}} to the preview and post the link.",
				trigger: {
					type: "hook",
					on: true,
					signature: {
						header: "x-hub-signature-256",
						encoding: "hex",
						prefix: "sha256=",
						has_secret: false,
					},
					rules: NO_RULES,
					hold: false,
				},
				target: { type: "agent", device: MAC, agent: "agent-release" },
				enabled: false,
				permission: "perm-task-deploy",
				made_by: {
					type: "tools",
					origin: {
						permission: "perm-release",
						by: { kind: "agent", agent: "agent-release" },
						device: MAC,
					},
				},
				next_run: null,
				last_run: null,
			},
		},
		{
			home: DEVBOX,
			task: {
				id: "task-triage",
				name: "Triage new issues",
				prompt: "Triage this issue and label it:\n{{body.issue.title}}\n{{body.issue.body}}",
				trigger: {
					type: "hook",
					on: true,
					signature: {
						header: "x-hub-signature-256",
						encoding: "hex",
						prefix: "sha256=",
						has_secret: true,
					},
					rules: {
						...NO_RULES,
						filters: [
							{
								path: "headers.x-github-event",
								test: { op: "equals", value: "issues" },
							},
							{
								path: "body.action",
								test: { op: "in", values: ["opened", "reopened"] },
							},
						],
						replay_key: "{{headers.x-github-delivery}}",
						max_age_minutes: 60,
					},
					hold: false,
				},
				target: {
					type: "new",
					device: DEVBOX,
					spec: {
						vendor: "codex",
						account: DEVBOX_CODEX,
						place: { type: "workspace", workspace: "ws-alpha" },
						model: null,
						permission: "full_access",
						title: null,
						first: null,
						branch: null,
					},
				},
				enabled: true,
				permission: "perm-task-triage",
				made_by: { type: "user" },
				next_run: null,
				last_run: { at: now - 40 * MIN, outcome: "done" },
			},
		},
		{
			home: DEVBOX,
			task: {
				id: "task-reviews",
				name: "Review pull requests",
				prompt: "Review this pull request and leave comments:\n{{body.pull_request.html_url}}",
				trigger: {
					type: "github_events",
					repo: "Example-User/MaxHQ",
					events: ["pull_request"],
					rules: {
						...NO_RULES,
						filters: [{ path: "body.action", test: { op: "equals", value: "opened" } }],
						daily_runs: 20,
					},
				},
				target: {
					type: "new",
					device: DEVBOX,
					spec: {
						vendor: "claude",
						account: DEVBOX_CLAUDE,
						place: { type: "workspace", workspace: MHQ_WORKSPACE },
						model: null,
						permission: "full_access",
						title: null,
						first: null,
						branch: null,
					},
				},
				enabled: true,
				permission: "perm-task-reviews",
				made_by: { type: "user" },
				next_run: null,
				last_run: { at: now - 3 * HOUR, outcome: "running" },
			},
		},
	];
};

const run = (task: TaskId, fields: Partial<Run> & Pick<Run, "cause" | "started">): Run => ({
	task,
	ended: fields.started + 6 * MIN,
	outcome: "done",
	agent: null,
	delivery: null,
	error: null,
	...fields,
});

const delivery = (
	task: TaskId,
	id: string,
	received: Timestamp,
	fields: Partial<TaskDelivery> = {}
): TaskDelivery => ({
	id,
	task,
	received,
	method: "POST",
	content_type: "application/json",
	body_bytes: 4812,
	outcome: "accepted",
	signature_verified: true,
	missing_fields: [],
	error: null,
	...fields,
});

const sampleRuns = (now: Timestamp): Run[] => [
	run("task-morning", { cause: "timer", started: now - 20 * HOUR }),
	run("task-morning", {
		cause: "timer",
		started: now - 44 * HOUR,
		ended: null,
		outcome: "missed",
		error: "The device was asleep or off.",
	}),
	run("task-checklist", {
		cause: "timer",
		started: now - 18 * MIN,
		outcome: "failed",
		agent: "agent-release",
		error: "The agent was waiting for an answer, so it couldn't take the message.",
	}),
	run("task-checklist", { cause: "now", started: now - 2 * HOUR, agent: "agent-release" }),
	run("task-triage", {
		cause: "hook",
		started: now - 40 * MIN,
		agent: "agent-gtfs",
		delivery: "del-triage-3",
	}),
	run("task-reviews", {
		cause: "github",
		started: now - 3 * HOUR,
		ended: null,
		outcome: "running",
		agent: "agent-egress",
		delivery: "51234567890",
	}),
];

const sampleDeliveries = (now: Timestamp): TaskDelivery[] => [
	delivery("task-triage", "del-triage-3", now - 40 * MIN),
	delivery("task-triage", "del-triage-2", now - 41 * MIN, { outcome: "duplicate" }),
	delivery("task-triage", "del-triage-1", now - 2 * HOUR, { outcome: "filtered" }),
	delivery("task-triage", "del-triage-0", now - DAY, {
		outcome: "rejected_signature",
		signature_verified: false,
		error: "The x-hub-signature-256 header didn't match the body.",
	}),
	delivery("task-reviews", "51234567890", now - 3 * HOUR, { method: "GET", body_bytes: 2210 }),
	delivery("task-reviews", "51234567001", now - 5 * HOUR, {
		method: "GET",
		body_bytes: 1904,
		outcome: "filtered",
		missing_fields: ["body.pull_request.html_url"],
	}),
];

const notFound = (id: TaskId): ActionResult => ({
	status: "error",
	error: { kind: "not_found", what: `task ${id}` },
});

const failed = (message: string): ActionResult => ({
	status: "error",
	error: { kind: "failed", message },
});

const BASE64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/** A sample token, as long as `maxd`'s 32 bytes in base64url. Not secret. */
const token = (): string =>
	Array.from({ length: 43 }, () => BASE64URL[Math.floor(Math.random() * 64)]).join("");

/** The sample tasks, and the entities that carry them. */
export type MockTasks = {
	readonly calls: TaskCalls;
	/** Each device's `device_tasks` entity as it stands. */
	readonly entities: () => Entity[];
};

/**
 * The sample tasks, kept in memory. `changed` runs after each change, so
 * the caller can publish the new entities.
 */
export const mockTasks = ({
	now,
	changed = () => {},
}: {
	now: () => Timestamp;
	changed?: () => void;
}): MockTasks => {
	const homes = new Map<TaskId, DeviceId>();
	const tasks = new Map<TaskId, Task>();
	for (const { home, task } of sampleTasks(now())) {
		homes.set(task.id, home);
		tasks.set(task.id, task);
	}
	const runs: Run[] = sampleRuns(now());
	const deliveries: TaskDelivery[] = sampleDeliveries(now());

	const on = (device: DeviceId) => [...tasks.values()].filter((t) => homes.get(t.id) === device);
	const addresses = new Map<DeviceId, HookAddress>();
	const address = (device: DeviceId): HookAddress => addresses.get(device) ?? { kind: "funnel" };

	/** Where `device`'s hook URLs start. */
	const base = (device: DeviceId): string => {
		const picked = address(device);
		if (picked.kind === "local") return `http://127.0.0.1:${PORT}`;
		return picked.kind === "own" ? picked.base : FUNNEL_HOST[device]!;
	};

	const funnel = (device: DeviceId): FunnelState => {
		if (address(device).kind !== "funnel") return { state: "on", url: base(device) };
		const listening = on(device).some((t) => t.trigger.type === "hook" && t.trigger.on);
		if (!listening) return { state: "off" };
		return device === DEVBOX ? { state: "on", url: FUNNEL_HOST[DEVBOX]! } : MAC_SETUP;
	};

	const webhooks = (device: DeviceId): TaskWebhook[] =>
		address(device).kind === "local"
			? []
			: on(device)
					.filter((t) => t.enabled && t.trigger.type === "github_events")
					.map((t) => ({ task: t.id, instant: true, last_delivery: now() - 12 * MIN }));

	const deviceTasks = (device: DeviceId): DeviceTasks => ({
		device,
		tasks: on(device),
		address: address(device),
		port: PORT,
		funnel: funnel(device),
		problems: [],
		webhooks: webhooks(device),
	});

	const hookUrl = (id: TaskId): string => `${base(homes.get(id)!)}/maxhq/${id}/${token()}`;

	const put = (task: Task) => {
		tasks.set(task.id, task);
		changed();
	};

	const withHook = (
		id: TaskId,
		edit: (hook: Extract<Trigger, { type: "hook" }>) => Extract<Trigger, { type: "hook" }>
	): ActionResult | null => {
		const task = tasks.get(id);
		if (!task) return notFound(id);
		if (task.trigger.type !== "hook") return failed("That task has no webhook.");
		put({ ...task, trigger: edit(task.trigger) });
		return null;
	};

	const calls: TaskCalls = {
		listTasks: async (device) => ({
			status: "ok",
			data: device === null ? [...tasks.values()] : on(device),
		}),
		saveTask: async (device, task) => {
			if (!DEVICES.includes(device))
				return { status: "error", error: { kind: "not_found", what: `device ${device}` } };
			const saved = tasks.get(task.id);
			const isNew = !saved;
			// The device keeps what it sets: whether a secret is held, and the URL's token.
			const kept =
				saved?.trigger.type === "hook" && task.trigger.type === "hook"
					? saved.trigger.signature?.has_secret === true
					: false;
			const trigger: Trigger =
				task.trigger.type === "hook" && task.trigger.signature
					? {
							...task.trigger,
							signature: { ...task.trigger.signature, has_secret: kept },
						}
					: task.trigger;
			const stored: Task = {
				...task,
				trigger,
				permission: saved?.permission ?? `perm-${task.id}`,
				next_run: nextRun({ ...task, trigger }, now()),
			};
			homes.set(task.id, device);
			put(stored);
			const madeHook =
				stored.trigger.type === "hook" && (isNew || saved.trigger.type !== "hook");
			return {
				status: "ok",
				data: {
					status: "task_saved",
					task: stored,
					hook_url: madeHook ? hookUrl(task.id) : null,
				},
			};
		},
		setTaskEnabled: async (device, id, enabled) => {
			const task = tasks.get(id);
			if (!task || homes.get(id) !== device) return notFound(id);
			const stored: Task = { ...task, enabled };
			put({ ...stored, next_run: nextRun(stored, now()) });
			return {
				status: "ok",
				data: { status: "task_saved", task: tasks.get(id)!, hook_url: null },
			};
		},
		deleteTask: async (id) => {
			if (!tasks.delete(id)) return notFound(id);
			homes.delete(id);
			changed();
			return DONE;
		},
		runTaskNow: async (id) => {
			const task = tasks.get(id);
			if (!task) return notFound(id);
			const at = now();
			runs.unshift({
				task: id,
				cause: "now",
				started: at,
				ended: null,
				outcome: "running",
				agent: task.target.type === "agent" ? task.target.agent : null,
				delivery: null,
				error: null,
			});
			put({ ...task, last_run: { at, outcome: "running" } });
			return DONE;
		},
		setHookOn: async (id, hookOn) => withHook(id, (hook) => ({ ...hook, on: hookOn })) ?? DONE,
		rotateHookToken: async (id) => {
			const task = tasks.get(id);
			if (!task) return notFound(id);
			if (task.trigger.type !== "hook") return failed("That task has no webhook.");
			return { status: "ok", data: { status: "hook_url", url: hookUrl(id) } };
		},
		setHookSecret: async (id, secret) => {
			if (!secret.trim()) return failed("The secret is empty.");
			return (
				withHook(id, (hook) => ({
					...hook,
					signature: hook.signature && { ...hook.signature, has_secret: true },
				})) ?? DONE
			);
		},
		setHookAddress: async (device, picked) => {
			if (!DEVICES.includes(device))
				return { status: "error", error: { kind: "not_found", what: `device ${device}` } };
			addresses.set(
				device,
				picked.kind === "own"
					? { kind: "own", base: picked.base.trim().replace(/\/+$/, "") }
					: picked
			);
			changed();
			return DONE;
		},
		listRuns: async (id) =>
			tasks.has(id)
				? { status: "ok", data: runs.filter((r) => r.task === id) }
				: { status: "error", error: { kind: "not_found", what: `task ${id}` } },
		listDeliveries: async (id) =>
			tasks.has(id)
				? { status: "ok", data: deliveries.filter((d) => d.task === id) }
				: { status: "error", error: { kind: "not_found", what: `task ${id}` } },
		funnelState: async (device) => ({ status: "ok", data: funnel(device) }),
	};

	return {
		calls,
		entities: () =>
			DEVICES.map((d) => ({ entity: "device_tasks" as const, ...deviceTasks(d) })),
	};
};
