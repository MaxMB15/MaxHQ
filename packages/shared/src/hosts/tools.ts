// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The tools a device's maxd installs, as rows: what each is, where it
// stands in an install, how much has downloaded, and the licences an
// install waits on.

import { t } from "../i18n";

import type {
	DeviceId,
	HelperTool,
	PendingLicence,
	ToolInfo,
	ToolKind,
	ToolPhase,
	ToolPlace,
} from "../bindings";
import { formatBytes } from "../agent-view/format";
import { TOOL_NAME } from "../fleet/update-view";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** Whether a device's `maxd` installs and removes tools itself. */
export const offersToolInstalls = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.tool_installs === true;

/** What the views call each helper. */
export const HELPER_NAME: Readonly<Record<HelperTool, string>> = {
	get node() {
		return t("Node.js");
	},
	uv: "uv",
	get gh() {
		return t("GitHub CLI");
	},
	agent_device: "agent-device",
	get jdk() {
		return t("Java");
	},
	get android_sdk() {
		return t("Android SDK");
	},
};

/** What an install phase is called on its row. */
export const PHASE_LABEL: Readonly<Record<ToolPhase, string>> = {
	get resolving() {
		return t("Finding the release");
	},
	get downloading() {
		return t("Downloading");
	},
	get verifying() {
		return t("Checking the download");
	},
	get installing() {
		return t("Installing");
	},
	get linking() {
		return t("Finishing");
	},
};

/** What a tool is called, with an ACP agent named from the registry when it's there. */
export const toolName = (kind: ToolKind, names?: ReadonlyMap<string, string>): string => {
	switch (kind.type) {
		case "agent":
			return TOOL_NAME[kind.tool];
		case "helper":
			return HELPER_NAME[kind.tool];
		case "acp":
			return names?.get(kind.id) ?? kind.id;
	}
};

/** Whether two kinds are the same tool. */
export const sameKind = (a: ToolKind, b: ToolKind): boolean => {
	if (a.type === "acp") return b.type === "acp" && a.id === b.id;
	return b.type !== "acp" && a.type === b.type && a.tool === b.tool;
};

/** Whether two places are the same. */
export const samePlace = (a: ToolPlace, b: ToolPlace): boolean =>
	a.type === "host" ? b.type === "host" : b.type === "workspace" && a.workspace === b.workspace;

/** Where a running install is. */
export type InstallProgress = {
	/** Such as "Downloading 12 MB of 80 MB", or the latest line. */
	readonly text: string;
	/** From 0 to 1 while the size is known, else none. */
	readonly fraction: number | null;
};

/** A running install's progress, or none when the tool isn't installing. */
export const installProgress = (tool: ToolInfo): InstallProgress | null => {
	if (tool.state.type !== "running") return null;
	const { phase, message } = tool.state;
	const done = tool.state.bytes_done ?? null;
	const total = tool.state.bytes_total ?? null;
	if (!phase) return { text: message, fraction: null };
	const label = PHASE_LABEL[phase];
	if (phase === "downloading" && done !== null) {
		const fraction = total ? Math.min(1, done / total) : null;
		const size = total
			? t("{{value1}} of {{value2}}", {
					value1: formatBytes(done),
					value2: formatBytes(total),
				})
			: formatBytes(done);
		return { text: `${label} ${size}`, fraction };
	}
	return { text: label, fraction: null };
};

/** One tool's row in a device's install list. */
export type ToolRow = {
	readonly tool: ToolInfo;
	readonly name: string;
	/** In words: installed, installing, waiting on licences, failed or missing. */
	readonly status: string;
	readonly progress: InstallProgress | null;
};

const statusOf = (tool: ToolInfo): string => {
	switch (tool.state.type) {
		case "running":
			return t("Installing");
		case "licences":
			return t("Waiting for you to accept its licences");
		case "failed":
			return t("Failed: {{tool_state_reason}}", { tool_state_reason: tool.state.reason });
		case "idle":
			return tool.version ? tool.version : t("Not installed");
	}
};

/**
 * The rows for a device's helpers and ACP agents, the tools `maxd`
 * installs beside the agent CLIs, in the order the device lists them.
 */
export const installRows = (
	view: ViewState,
	device: DeviceId,
	names?: ReadonlyMap<string, string>
): ToolRow[] =>
	(view.tables.device_tools.get(device)?.tools ?? [])
		.filter((tool) => tool.kind.type !== "agent")
		.map((tool) => ({
			tool,
			name: toolName(tool.kind, names),
			status: statusOf(tool),
			progress: installProgress(tool),
		}));

/** A licence an install waits on, with the tool and place it's for. */
export type LicenceAsk = {
	readonly device: DeviceId;
	readonly tool: ToolInfo;
	readonly licence: PendingLicence;
};

/** Every licence an install waits on, across the fleet. */
export const pendingLicences = (view: ViewState): LicenceAsk[] =>
	[...view.tables.device_tools.values()].flatMap(({ device, tools }) =>
		tools.flatMap((tool) =>
			tool.state.type === "licences"
				? tool.state.pending.map((licence) => ({ device, tool, licence }))
				: []
		)
	);

/**
 * The helpers a device doesn't list on itself yet, to offer Install for.
 * The Android SDK installs on a Mac only.
 */
export const missingHelpers = (view: ViewState, device: DeviceId): HelperTool[] => {
	const listed = view.tables.device_tools.get(device)?.tools ?? [];
	const mac = view.tables.device.get(device)?.os === "macos";
	return (Object.keys(HELPER_NAME) as HelperTool[]).filter(
		(helper) =>
			(mac || helper !== "android_sdk") &&
			!listed.some(
				(t) => t.place.type === "host" && t.kind.type === "helper" && t.kind.tool === helper
			)
	);
};
