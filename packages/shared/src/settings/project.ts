// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's settings on one device as the form holds them, and the
// message a save sends: whether agents resume after a restart, the ports
// never offered, the commit author in its containers, the idle days before
// its agents settle, when they differ from the device's, and whether an
// agent settles when its PR merges, and each container's soft memory limit
// when it differs from the host spec's. The author defaults to this Mac's
// global git identity.

import { t } from "../i18n";

import type {
	DeviceId,
	GitAuthor,
	GitIdentity,
	ProjectId,
	ProjectSettings,
	Timestamp,
} from "../bindings";
import { formatAgo } from "../status/format";
import { MAX_SETTLE_DAYS, parseSettleDays } from "./inbox";
import { all, type ViewState } from "../view/replica";
import { deviceName } from "../view/select";
import { workspaceHosts } from "../workspaces/state";

/** What the form holds, as typed. */
export type ProjectDraft = {
	readonly resume: boolean;
	readonly ports: string;
	readonly name: string;
	readonly email: string;
	/** The idle days before auto-settle, or empty for the device's. */
	readonly autoSettle: string;
	/** Whether an agent settles when its PR merges. */
	readonly settleOnMerge: boolean;
	/** Whether agents get the variables a workflow's descriptor names. */
	readonly workflowEnv: boolean;
	/** Each container's soft memory limit in GiB, or empty for the host spec's. */
	readonly memory: string;
};

const GIB = 1024 ** 3;

/** The largest soft memory limit the form takes, in GiB. */
export const MAX_MEMORY_GIB = 1024;

/** The settings a device keeps for the project, if it keeps any. */
export const keptSettingsOn = (
	view: ViewState,
	project: ProjectId,
	device: DeviceId
): ProjectSettings | undefined =>
	all(view, "project_settings").find((s) => s.project === project && s.device === device);

/** The settings a device keeps for the project, or the defaults when it keeps none. */
export const projectSettingsOn = (
	view: ViewState,
	project: ProjectId,
	device: DeviceId
): ProjectSettings =>
	keptSettingsOn(view, project, device) ?? {
		project,
		device,
		resume_on_restart: true,
		ignored_ports: [],
		git_author: null,
	};

/**
 * The author git names for this Mac, or null unless it has both a name and
 * an email. Half of one would only start the form on an error.
 */
export const defaultAuthor = (identity: GitIdentity | undefined): GitAuthor | null => {
	const name = identity?.name?.trim() ?? "";
	const email = identity?.email?.trim() ?? "";
	return name !== "" && email !== "" ? { name, email } : null;
};

/**
 * What a device that keeps nothing for the project starts with: the
 * defaults, with this Mac's git identity as the author. Null when the device
 * keeps settings, even ones with the author cleared, or when git has no
 * whole identity to give.
 */
export const firstSettingsOn = (
	view: ViewState,
	project: ProjectId,
	device: DeviceId,
	identity: GitIdentity | undefined
): ProjectSettings | null => {
	if (keptSettingsOn(view, project, device)) return null;
	const author = defaultAuthor(identity);
	return author ? { ...projectSettingsOn(view, project, device), git_author: author } : null;
};

/**
 * The workflow variables a device keeps as allowed for a project's agents,
 * as the setting says it, or null while it keeps none.
 */
export const allowedWorkflowEnvText = (
	settings: ProjectSettings,
	now: Timestamp
): string | null => {
	const allowed = settings.workflow_env_allowed;
	if (!allowed || settings.workflow_env === false || allowed.vars.length === 0) return null;
	const names = allowed.vars.map((v) => v.name).join(", ");
	return t(
		"You allowed {{names}} for every agent here {{formatAgo_allowed_at__now}}. Turn this off and save to forget it.",
		{ names: names, formatAgo_allowed_at__now: formatAgo(allowed.at, now) }
	);
};

/** The form's fields for settings a device keeps. */
export const draftOf = (settings: ProjectSettings): ProjectDraft => ({
	resume: settings.resume_on_restart,
	ports: settings.ignored_ports.join(", "),
	name: settings.git_author?.name ?? "",
	email: settings.git_author?.email ?? "",
	autoSettle: settings.auto_settle_days == null ? "" : String(settings.auto_settle_days),
	settleOnMerge: settings.settle_on_merge ?? true,
	workflowEnv: settings.workflow_env ?? true,
	memory:
		settings.memory_soft == null ? "" : String(Number((settings.memory_soft / GIB).toFixed(2))),
});

/** Whether two drafts say the same. */
export const sameDraft = (a: ProjectDraft, b: ProjectDraft): boolean =>
	a.resume === b.resume &&
	a.ports === b.ports &&
	a.name === b.name &&
	a.email === b.email &&
	a.autoSettle === b.autoSettle &&
	a.settleOnMerge === b.settleOnMerge &&
	a.workflowEnv === b.workflowEnv &&
	a.memory === b.memory;

/** The ports typed in, in order with no repeats, or why they don't read as ports. */
export const parsePorts = (
	text: string
): { ok: true; ports: number[] } | { ok: false; reason: string } => {
	const parts = text.split(/[\s,]+/).filter(Boolean);
	const ports = new Set<number>();
	for (const part of parts) {
		const port = /^\d{1,5}$/.test(part) ? Number(part) : NaN;
		if (!(port >= 1 && port <= 65535)) {
			return {
				ok: false,
				reason: t("{{part}} isn't a port. Ports run from 1 to 65535.", { part: part }),
			};
		}
		ports.add(port);
	}
	return { ok: true, ports: [...ports].sort((a, b) => a - b) };
};

/**
 * The soft memory limit typed in, in bytes, null when empty for the host
 * spec's, or why it doesn't read as one.
 */
export const parseMemory = (
	text: string
): { ok: true; bytes: number | null } | { ok: false; reason: string } => {
	const typed = text.trim().replace(/\s*gib?$/i, "");
	if (typed === "") return { ok: true, bytes: null };
	const gib = /^\d+(\.\d+)?$/.test(typed) ? Number(typed) : NaN;
	if (!(gib > 0 && gib <= MAX_MEMORY_GIB)) {
		return {
			ok: false,
			reason: t(
				"{{text_trim}} isn't a memory limit. Use GiB above 0 and up to {{MAX_MEMORY_GIB}}, or leave it empty for the host's.",
				{ text_trim: text.trim(), MAX_MEMORY_GIB: MAX_MEMORY_GIB }
			),
		};
	}
	return { ok: true, bytes: Math.round(gib * GIB) };
};

/** The message a save sends, or why the form can't send it. */
export type ProjectPlan =
	| { readonly ok: true; readonly settings: ProjectSettings }
	| { readonly ok: false; readonly reason: string };

/** Builds the message from what the form holds. */
export const planProjectSettings = (
	project: ProjectId,
	device: DeviceId,
	draft: ProjectDraft
): ProjectPlan => {
	const ports = parsePorts(draft.ports);
	if (!ports.ok) return ports;
	const name = draft.name.trim();
	const email = draft.email.trim();
	if (name === "" && email !== "") return { ok: false, reason: t("Give the author a name too.") };
	if (name !== "" && email === "")
		return { ok: false, reason: t("Give the author an email too.") };
	if (email !== "" && !/^[^\s@]+@[^\s@]+$/.test(email)) {
		return { ok: false, reason: t("{{email}} isn't an email address.", { email: email }) };
	}
	const typed = draft.autoSettle.trim();
	const days = typed === "" ? null : parseSettleDays(typed);
	if (days && !days.ok) {
		return {
			ok: false,
			reason: t(
				"{{typed}} isn't a number of days. Use 0 to {{MAX_SETTLE_DAYS}}, or leave it empty for the device's.",
				{ typed: typed, MAX_SETTLE_DAYS: MAX_SETTLE_DAYS }
			),
		};
	}
	const memory = parseMemory(draft.memory);
	if (!memory.ok) return memory;
	return {
		ok: true,
		settings: {
			project,
			device,
			resume_on_restart: draft.resume,
			ignored_ports: ports.ports,
			git_author: name === "" ? null : { name, email },
			auto_settle_days: days ? days.days : null,
			settle_on_merge: draft.settleOnMerge,
			workflow_env: draft.workflowEnv,
			memory_soft: memory.bytes,
		},
	};
};

/**
 * The devices a project has settings on: those that host workspaces and any
 * other that already keeps some, by name.
 */
export const projectDevices = (view: ViewState, project: ProjectId): DeviceId[] => {
	const devices = new Set(workspaceHosts(view));
	for (const s of all(view, "project_settings")) {
		if (s.project === project) devices.add(s.device);
	}
	return [...devices].sort((a, b) => deviceName(view, a).localeCompare(deviceName(view, b)));
};
