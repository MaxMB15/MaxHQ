// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where tasks live: which devices run them, the device a new task's home
// defaults to, the repos a GitHub events task can watch, the agents a task
// can send its prompt to, and the tasks that reach one agent, which keep it
// from auto-settling while one is enabled. A device
// whose `maxd` predates scheduled tasks shows none of it.

import { t } from "../i18n";

import type { AgentId, DeviceId, DeviceTasks, Task } from "../bindings";
import type { Option } from "../new-agent/spec";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";
import type { TriggerKind } from "./draft";

/** Whether a device's `maxd` runs scheduled tasks. */
export const runsTasks = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.scheduled_tasks === true;

const byName = (view: ViewState) => (a: DeviceId, b: DeviceId) =>
	deviceName(view, a).localeCompare(deviceName(view, b));

/** The devices that run scheduled tasks, by name. */
export const taskDevices = (view: ViewState): DeviceId[] =>
	all(view, "device")
		.map((d) => d.id)
		.filter((d) => runsTasks(view, d))
		.sort(byName(view));

/** Each device's tasks and Funnel, for the devices that run tasks, by name. */
export const deviceTasks = (view: ViewState): DeviceTasks[] =>
	taskDevices(view).map(
		(device) =>
			view.tables.device_tasks.get(device) ?? {
				device,
				tasks: [],
				address: { kind: "local" },
				port: null,
				funnel: { state: "off" },
				problems: [],
			}
	);

const connected = (view: ViewState, device: DeviceId) =>
	linkForDevice(view, device)?.state.state === "connected";

/**
 * The device that stays on, for a webhook to reach: the first connected
 * one that runs tasks and isn't a Mac, since a laptop sleeps.
 */
export const alwaysOn = (view: ViewState): DeviceId | null =>
	taskDevices(view).find(
		(d) => connected(view, d) && view.tables.device.get(d)?.os !== "macos"
	) ?? null;

/** The repos each device's GitHub App reaches. */
export const githubRepos = (view: ViewState): { repo: string; device: DeviceId }[] =>
	all(view, "credential")
		.flatMap((c) =>
			c.kind.type === "github_app" && runsTasks(view, c.device)
				? c.kind.repos.map((repo) => ({ repo, device: c.device }))
				: []
		)
		.sort((a, b) => a.repo.localeCompare(b.repo) || byName(view)(a.device, b.device));

/**
 * Where a new task lives: the target's device for a timer, the device that
 * stays on for a webhook, and for GitHub events a device whose App
 * reaches the repo, the one that stays on first.
 */
export const defaultHome = (
	view: ViewState,
	kind: TriggerKind,
	target: DeviceId | null,
	repo = ""
): DeviceId | null => {
	const devices = taskDevices(view);
	const own = target && devices.includes(target) ? target : null;
	const on = alwaysOn(view);
	if (kind === "every" || kind === "at") return own ?? on ?? devices[0] ?? null;
	if (kind === "github_events") {
		const reach = githubRepos(view)
			.filter((r) => r.repo === repo)
			.map((r) => r.device);
		if (reach.length > 0) return on && reach.includes(on) ? on : reach[0]!;
	}
	return on ?? own ?? devices[0] ?? null;
};

/** The choices for a task's home. */
export const homeOptions = (view: ViewState): Option[] =>
	taskDevices(view).map((d) => ({
		value: d,
		label: deviceName(view, d),
		disabled: !connected(view, d),
		detail: connected(view, d) ? undefined : t("Not connected"),
	}));

/**
 * The agents MaxHQ holds, which a task can send its prompt to, and `kept`,
 * the one a task reaches already, which an agent's task may name.
 */
export const agentOptions = (view: ViewState, kept: AgentId | null = null): Option[] =>
	all(view, "agent")
		.filter((a) => a.id === kept || (a.control === "held" && a.parent === null))
		.map((a) => {
			const device = view.tables.workspace.get(a.workspace)?.device;
			return {
				value: a.id,
				label: a.title,
				detail: device ? deviceName(view, device) : undefined,
			};
		})
		.sort((a, b) => a.label.localeCompare(b.label));

/** A task with the device it lives on. */
export type HomedTask = { readonly task: Task; readonly home: DeviceId };

/** The tasks whose runs reach an agent. */
export const tasksTargeting = (view: ViewState, agent: AgentId): HomedTask[] =>
	deviceTasks(view).flatMap(({ device, tasks }) =>
		tasks
			.filter((t) => t.target.type === "agent" && t.target.agent === agent)
			.map((task) => ({ task, home: device }))
	);

/**
 * Whether an enabled task sends its runs to the agent, which keeps it
 * from auto-settling: its device leaves it be, whatever its switch says.
 */
export const keptByAutomation = (view: ViewState, agent: AgentId): boolean =>
	tasksTargeting(view, agent).some(({ task }) => task.enabled);

/** The device a task lives on, if a device that runs tasks holds it. */
export const homeOf = (view: ViewState, task: string): DeviceId | undefined =>
	deviceTasks(view).find((d) => d.tasks.some((t) => t.id === task))?.device;

/**
 * The warning for a timed task on a Mac: it doesn't wake to run one, so a
 * time it sleeps through is missed.
 */
export const sleepWarning = (
	view: ViewState,
	kind: TriggerKind,
	home: DeviceId | null
): string | null =>
	kind !== "every" && kind !== "at"
		? null
		: home && view.tables.device.get(home)?.os === "macos"
			? t(
					"{{deviceName_view__home}} doesn't wake up to run it. A run it sleeps through is missed.",
					{ deviceName_view__home: deviceName(view, home) }
				)
			: null;
