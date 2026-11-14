// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the capsule's words,
// the elapsed time, the status labels, closing a run once every device is
// done, dismissing the settled ones, and the plan's summary. MaxHQ's run
// follows the `FleetProgress` the app sends instead of each server's own
// update state, and a device is behind when its build isn't the one the app
// carries.

import { t } from "../i18n";

import { SIGNED_UPDATES_SINCE, type DeviceId, type Timestamp } from "../bindings";
import type { ViewState } from "../view/replica";
import type {
	BundledBuild,
	Device,
	FleetProgress,
	FleetStatus,
	Tool,
	ToolInfo,
	ToolTarget,
} from "./wire";

/** What the sidebar and the device detail call each CLI. */
export const TOOL_NAME: Readonly<Record<Tool, string>> = {
	claude: "Claude",
	codex: "Codex",
	cursor: "Cursor",
};

/** An agent CLI's row in a device's tools. */
export type AgentToolInfo = ToolInfo & { readonly kind: { type: "agent"; tool: Tool } };

/** Whether a tool is an agent CLI rather than a helper or an ACP agent. */
export const isAgentTool = (tool: ToolInfo): tool is AgentToolInfo => tool.kind.type === "agent";

/** The agent CLIs among a device's tools, the only ones the views show yet. */
export const agentTools = (tools: readonly ToolInfo[]): AgentToolInfo[] =>
	tools.filter(isAgentTool);

/**
 * Whether a device runs a different `maxd` than the one the app carries
 * for its target. A device or app that names no build is never behind,
 * since there's nothing to compare.
 */
export const isBehind = (device: Device, bundled: readonly BundledBuild[] | null): boolean => {
	if (!bundled || !device.build || !device.target) return false;
	const carried = bundled.find((b) => b.target === device.target);
	return carried !== undefined && carried.build !== device.build;
};

/**
 * Whether the `maxd` the app carries for a device's target is a build from
 * source, which no release signed. An app that names no signing, from
 * before signed updates, counts as unsigned.
 */
export const isUnsigned = (device: Device, bundled: readonly BundledBuild[] | null): boolean =>
	bundled?.find((b) => b.target === device.target)?.signed !== true;

/**
 * Whether a device's `maxd` checks the release signature on an update. One
 * from before signed updates installs what it's sent without a look.
 */
export const checksSignatures = (protocol: number | null): boolean =>
	protocol !== null && protocol >= SIGNED_UPDATES_SINCE;

/** Whether a CLI has a newer version than the one installed. */
export const hasNewer = (tool: ToolInfo): boolean =>
	tool.latest !== null && tool.version !== null && tool.latest !== tool.version;

/**
 * Whether MaxHQ may update a CLI now: one it installed, one you allowed,
 * or one on the host whose tool you ticked in the device's settings.
 */
export const mayUpdate = (tool: AgentToolInfo, ticked: readonly Tool[]): boolean => {
	switch (tool.update.type) {
		case "auto":
		case "allowed":
			return true;
		case "needs_tick":
			return ticked.includes(tool.kind.tool);
		case "manual":
		case "never":
			return false;
	}
};

/** The CLI and the place it's in, to send to `updateTools`. */
export const targetOf = (tool: AgentToolInfo): ToolTarget => ({
	tool: tool.kind.tool,
	place: tool.place,
});

/** One device in an Update everything plan. */
export type PlanRow = {
	readonly device: DeviceId;
	readonly label: string;
	/** Whether its `maxd` is behind the app's. */
	readonly maxd: boolean;
	/** Whether the `maxd` it would get is a build from source no release signed. */
	readonly unsigned: boolean;
	/** Whether it would get a `maxd` without checking its signature, being too old to. */
	readonly unchecked: boolean;
	/** The CLIs MaxHQ would update, by name and place. */
	readonly tools: readonly string[];
};

/** What Update everything would do. */
export type UpdatePlan = {
	/** Such as "Update 2 devices and 1 CLI", or null when nothing is behind. */
	readonly summary: string | null;
	readonly rows: readonly PlanRow[];
	/** Whether any CLI is in the plan, so the run asks for them. */
	readonly tools: boolean;
	/**
	 * Whether any device would get a `maxd` no release signed, so the run
	 * asks you first and sends it only once you agree.
	 */
	readonly unsigned: boolean;
};

/** A CLI by name, with the container it's in, such as "Codex in mhq-one". */
export const toolLabel = (view: ViewState, tool: AgentToolInfo): string => {
	const name = TOOL_NAME[tool.kind.tool];
	if (tool.place.type === "host") return name;
	const workspace = view.tables.workspace.get(tool.place.workspace);
	const where =
		workspace?.location.type === "container"
			? workspace.location.instance
			: tool.place.workspace;
	return t("{{value1}} in {{value2}}", { value1: name, value2: where });
};

/** The CLIs on a device that MaxHQ would update now. */
export const updatableTools = (view: ViewState, device: DeviceId): AgentToolInfo[] => {
	const tools = agentTools(view.tables.device_tools.get(device)?.tools ?? []);
	const ticked = view.tables.device_settings.get(device)?.tool_updates ?? [];
	return tools.filter((t) => hasNewer(t) && mayUpdate(t, ticked) && t.state.type !== "running");
};

/** The connected devices that are behind or have a CLI to update, by the sidebar's names. */
export const planUpdate = (
	view: ViewState,
	bundled: readonly BundledBuild[] | null
): UpdatePlan => {
	const rows: PlanRow[] = [];
	for (const link of view.tables.device_link.values()) {
		if (!link.device || link.state.state !== "connected") continue;
		const device = view.tables.device.get(link.device);
		if (!device) continue;
		const maxd = isBehind(device, bundled);
		const tools = updatableTools(view, link.device).map((t) => toolLabel(view, t));
		if (maxd || tools.length > 0)
			rows.push({
				device: link.device,
				label: link.name,
				maxd,
				unsigned: maxd && isUnsigned(device, bundled),
				unchecked: maxd && !checksSignatures(link.protocol ?? null),
				tools,
			});
	}
	rows.sort((a, b) => a.label.localeCompare(b.label));
	const devices = rows.filter((r) => r.maxd).length;
	const cli = rows.reduce((n, r) => n + r.tools.length, 0);
	const parts = [countLabel(devices, "device"), countLabel(cli, "CLI")].filter(
		(part) => part !== null
	);
	return {
		summary:
			parts.length === 0
				? null
				: t("Update {{joinList_parts}}", { joinList_parts: joinList(parts) }),
		rows,
		tools: cli > 0,
		unsigned: rows.some((r) => r.unsigned),
	};
};

/** One device in a run. */
export type RunDevice = {
	readonly device: DeviceId;
	readonly label: string;
	/** When its current try started, for the elapsed time. */
	readonly startedAt: Timestamp;
	readonly status: FleetStatus;
};

/** An Update everything run, from the moment you start it until you dismiss it. */
export type FleetRun = {
	/** Whether it updates CLIs as well, so a retry does too. */
	readonly tools: boolean;
	/** Whether you agreed to send a `maxd` no release signed, so a retry does too. */
	readonly unsigned: boolean;
	readonly devices: readonly RunDevice[];
};

type Status = FleetStatus["status"];

/** What the capsule's list calls each step. */
export const RUN_STATUS_LABELS: Readonly<Record<Status, string>> = {
	waiting: "waiting",
	get tools() {
		return t("updating CLIs");
	},
	uploading: "uploading",
	restarting: "restarting",
	get guests() {
		return t("updating workspaces");
	},
	done: "done",
	skipped: "skipped",
	failed: "failed",
};

/** Whether a step is still to finish. */
export const isActive = (status: FleetStatus): boolean =>
	status.status !== "done" && status.status !== "failed" && status.status !== "skipped";

/** A new run over these devices, each waiting. */
export const startRun = (
	rows: readonly Pick<PlanRow, "device" | "label">[],
	tools: boolean,
	unsigned: boolean,
	now: Timestamp
): FleetRun => ({
	tools,
	unsigned,
	devices: rows.map(({ device, label }) => ({
		device,
		label,
		startedAt: now,
		status: { status: "waiting" },
	})),
});

/** The run with one device's step. Leaving waiting starts its clock. */
export const applyProgress = (
	run: FleetRun,
	progress: FleetProgress,
	now: Timestamp
): FleetRun => ({
	...run,
	devices: run.devices.map((d) =>
		d.device === progress.device
			? {
					...d,
					status: progress.status,
					startedAt: d.status.status === "waiting" ? now : d.startedAt,
				}
			: d
	),
});

/** The run with these devices waiting again, for a retry. */
export const restartDevices = (
	run: FleetRun,
	devices: readonly DeviceId[],
	now: Timestamp
): FleetRun => ({
	...run,
	devices: run.devices.map((d) =>
		devices.includes(d.device) ? { ...d, startedAt: now, status: { status: "waiting" } } : d
	),
});

/** Null once every device is done, so a clean run leaves no capsule behind. */
export const closeFinishedRun = (run: FleetRun): FleetRun | null =>
	run.devices.every((d) => d.status.status === "done") ? null : run;

/** The run without its settled devices, or null when none is left. */
export const dismissSettled = (run: FleetRun): FleetRun | null => {
	const devices = run.devices.filter((d) => isActive(d.status));
	return devices.length === 0 ? null : { ...run, devices };
};

/** The devices whose update failed, for a retry. */
export const failedDevices = (run: FleetRun): DeviceId[] =>
	run.devices.filter((d) => d.status.status === "failed").map((d) => d.device);

const UPDATE_SLOW_AFTER_MS = 3 * 60 * 1000;

/** How long a step has run, rounded down to the minute. */
export const formatUpdateElapsed = (ms: number): string => {
	const minutes = Math.floor(ms / 60_000);
	if (minutes < 1) return "<1m";
	if (minutes < 60) return `${minutes}m`;
	return t("{{Math_floor_minutes___60}}h {{minutes___60}}m", {
		Math_floor_minutes___60: Math.floor(minutes / 60),
		minutes___60: minutes % 60,
	});
};

/** What the capsule shows. */
export type UpdateCapsuleView = {
	readonly tone: "default" | "slow" | "failed";
	readonly label: string;
	/** The slowest device's step and how long it has taken, once it's slow. */
	readonly slow?: { readonly status: string; readonly elapsed: string };
	readonly action: "retry" | "dismiss" | null;
};

/** The capsule's words for a run at `now`. */
export const describeUpdateCapsule = (run: FleetRun, now: Timestamp): UpdateCapsuleView => {
	const moving = run.devices.filter((d) => isActive(d.status) && d.status.status !== "waiting");
	const slowest = [...moving].sort((a, b) => a.startedAt - b.startedAt)[0];
	if (slowest && now - slowest.startedAt >= UPDATE_SLOW_AFTER_MS) {
		return {
			tone: "slow",
			label: slowest.label,
			slow: {
				status: RUN_STATUS_LABELS[slowest.status.status],
				elapsed: formatUpdateElapsed(now - slowest.startedAt),
			},
			action: null,
		};
	}
	const done = run.devices.filter((d) => d.status.status === "done").length;
	const counted = t("{{value1}} of {{value2}} updated", {
		value1: done,
		value2: run.devices.length,
	});
	if (run.devices.some((d) => isActive(d.status))) {
		return { tone: "default", label: counted, action: null };
	}
	const failed = run.devices.filter((d) => d.status.status === "failed");
	if (failed.length > 0) {
		return {
			tone: "failed",
			label:
				failed.length === 1
					? t("{{value1}} failed", { value1: failed[0]!.label })
					: t("{{value1}} failed", { value1: failed.length }),
			action: "retry",
		};
	}
	return { tone: "default", label: counted, action: "dismiss" };
};

/** What one device's line in the capsule's list says. */
export const describeRunDevice = (device: RunDevice, now: Timestamp): string => {
	const { status } = device;
	if (status.status === "skipped") return t("skipped: {{value1}}", { value1: status.reason });
	if (status.status === "failed") return "failed";
	const label = RUN_STATUS_LABELS[status.status];
	return isActive(status) && status.status !== "waiting"
		? `${label} ${formatUpdateElapsed(now - device.startedAt)}`
		: label;
};

const countLabel = (count: number, noun: string): string | null =>
	count === 0 ? null : `${count} ${noun}${count === 1 ? "" : "s"}`;

const joinList = (parts: readonly string[]): string =>
	parts.length <= 2
		? parts.join(" and ")
		: t("{{value1}}, and {{value2}}", {
				value1: parts.slice(0, -1).join(", "),
				value2: parts.at(-1),
			});
