// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a permission to run agents covers, as the forms edit it: which
// devices, vendors, accounts and places, how deep, the limits, and the
// kinds of test host it may lease. The
// allow sheet on an agent and the access key form share it, and the
// agent tools settings use the limits part for the defaults.

import { t } from "../i18n";

import type {
	AccountId,
	Actor,
	Agent,
	AgentDefaults,
	AgentId,
	AgentPlace,
	DeviceId,
	LeaveContainer,
	Limits,
	McpCommand,
	Permission,
	TestKind,
	Timestamp,
	Vendor,
} from "../bindings";
import { formatWhen, listInWords, plural } from "../status/format";
import { accountName } from "../status/usage";
import { all, type ViewState } from "../view/replica";
import { accounts, deviceName, workspaceName } from "../view/select";

const DAY = 24 * 60 * 60 * 1000;

/** The vendors a permission can start. */
export const VENDORS: readonly Vendor[] = ["claude", "codex"];

/** The most levels a permission may reach. */
export const MAX_DEPTH = 10;

/** The limits as their fields hold them, empty for off. */
export type LimitsDraft = {
	readonly started: string;
	readonly running: string;
	readonly usage: string;
	/** Days from now until it expires. */
	readonly days: string;
};

/** A permission's scope as the form holds it. Null accounts or places mean any. */
export type ScopeDraft = {
	readonly devices: readonly DeviceId[];
	readonly vendors: readonly Vendor[];
	readonly accounts: readonly AccountId[] | null;
	readonly places: readonly AgentPlace[] | null;
	readonly depth: string;
	readonly limits: LimitsDraft;
	/** The kinds of test host it may lease, none unless you pick them. */
	readonly testKinds: readonly TestKind[];
	/** Whether an agent in a container may reach agents outside it. Ask unless you pick another. */
	readonly leaveContainer: LeaveContainer;
};

const field = (value: number | null): string => (value === null ? "" : String(value));

/** The limits' fields. An expiry shows as the days left, rounded up. */
export const limitsDraft = (limits: Limits, now: Timestamp): LimitsDraft => ({
	started: field(limits.max_started),
	running: field(limits.max_running),
	usage: field(limits.usage_cap_percent),
	days:
		limits.expires_at === null
			? ""
			: String(Math.max(1, Math.ceil((limits.expires_at - now) / DAY))),
});

/** Either what the form means, or why it can't be saved. */
export type Parsed<T> = { readonly ok: T } | { readonly error: string };

// A whole number from `min` to `max`, null when empty, or undefined when it isn't one.
const whole = (text: string, min: number, max: number): number | null | undefined => {
	const trimmed = text.trim();
	if (trimmed === "") return null;
	if (!/^\d+$/.test(trimmed)) return undefined;
	const n = Number(trimmed);
	return n >= min && n <= max ? n : undefined;
};

/** The limits the fields say, each one off when its field is empty. */
export const limitsFrom = (draft: LimitsDraft, now: Timestamp): Parsed<Limits> => {
	const started = whole(draft.started, 1, 1_000_000);
	if (started === undefined)
		return { error: t("Agents started must be a whole number above 0.") };
	const running = whole(draft.running, 1, 1_000_000);
	if (running === undefined)
		return { error: t("Agents at once must be a whole number above 0.") };
	const usage = whole(draft.usage, 1, 100);
	if (usage === undefined) return { error: t("The usage cap must be a percent from 1 to 100.") };
	const days = whole(draft.days, 1, 3650);
	if (days === undefined)
		return { error: t("Days until it expires must be a whole number above 0.") };
	return {
		ok: {
			max_started: started,
			max_running: running,
			usage_cap_percent: usage,
			expires_at: days === null ? null : now + days * DAY,
		},
	};
};

/** The depth the field says, from 1 to MAX_DEPTH. */
export const depthFrom = (text: string): Parsed<number> => {
	const depth = whole(text, 1, MAX_DEPTH);
	return typeof depth === "number"
		? { ok: depth }
		: {
				error: t("Depth must be a whole number from 1 to {{MAX_DEPTH}}.", {
					MAX_DEPTH: MAX_DEPTH,
				}),
			};
};

/**
 * The form for a permission: the one there is, or a new one over every
 * device and vendor, any account and place, with the defaults and no test
 * hosts.
 */
export const scopeDraft = (
	permission: Permission | undefined,
	defaults: AgentDefaults,
	devices: readonly DeviceId[],
	now: Timestamp
): ScopeDraft =>
	permission
		? {
				devices: permission.devices,
				vendors: permission.vendors,
				accounts: permission.accounts,
				places: permission.places,
				depth: String(permission.depth),
				limits: limitsDraft(permission.limits ?? defaults.limits, now),
				testKinds: permission.test_kinds ?? [],
				leaveContainer: permission.leave_container ?? "ask",
			}
		: {
				devices,
				vendors: VENDORS,
				accounts: null,
				places: null,
				depth: String(defaults.depth),
				limits: limitsDraft(defaults.limits, now),
				testKinds: [],
				leaveContainer: "ask",
			};

/** The permission form's name for whether agents in a container may reach outside it. */
export const CONTAINERS = "Containers";

/** What the form offers for agents in a container, in its order. */
export const LEAVE_CONTAINER: readonly Choice<LeaveContainer>[] = [
	{
		value: "ask",
		get label() {
			return t("Ask");
		},
	},
	{
		value: "allow",
		get label() {
			return t("Always allow");
		},
	},
	{
		value: "never",
		get label() {
			return t("Never");
		},
	},
];

/** The permission a form says, for `holder`, counted on `home`. */
export const permissionFrom = (
	draft: ScopeDraft,
	who: { readonly id: string; readonly holder: Actor; readonly home: DeviceId },
	now: Timestamp
): Parsed<Permission> => {
	if (draft.devices.length === 0) return { error: t("Pick at least one device.") };
	if (draft.vendors.length === 0) return { error: t("Pick at least one vendor.") };
	if (draft.accounts?.length === 0)
		return { error: t("Pick at least one account, or allow any.") };
	if (draft.places?.length === 0) return { error: t("Pick at least one place, or allow any.") };
	const depth = depthFrom(draft.depth);
	if ("error" in depth) return depth;
	const limits = limitsFrom(draft.limits, now);
	if ("error" in limits) return limits;
	return {
		ok: {
			id: who.id,
			holder: who.holder,
			home: who.home,
			devices: [...draft.devices],
			vendors: [...draft.vendors],
			accounts: draft.accounts && [...draft.accounts],
			places: draft.places && [...draft.places],
			depth: depth.ok,
			limits: limits.ok,
			test_kinds: [...draft.testKinds],
			leave_container: draft.leaveContainer,
		},
	};
};

/**
 * Whether an agent can hold a permission: one MaxHQ runs, which gets the
 * tools over its pipe, and not a sub-agent. One watched in a terminal
 * can't call them as itself.
 */
export const canAllow = (agent: Pick<Agent, "control" | "parent">): boolean =>
	agent.control === "held" && agent.parent === null;

/** The permission an agent holds, if it holds one. */
export const heldBy = (
	permissions: readonly Permission[],
	agent: AgentId
): Permission | undefined =>
	permissions.find((p) => p.holder.kind === "agent" && p.holder.agent === agent);

/** Whether one place is the same as another. */
export const samePlace = (a: AgentPlace, b: AgentPlace): boolean =>
	a.type === "workspace"
		? b.type === "workspace" && a.workspace === b.workspace
		: b.type === "folder" && a.path === b.path;

/** One account or place a form offers. */
export type Choice<T> = { readonly value: T; readonly label: string };

/** The accounts a form offers: those of the vendors it picked, on the devices it picked. */
export const accountChoices = (view: ViewState, draft: ScopeDraft): Choice<AccountId>[] =>
	accounts(view)
		.filter((a) => draft.vendors.includes(a.vendor) && draft.devices.includes(a.device))
		.map((a) => ({
			value: a.id,
			label: t("{{value1}} on {{value2}}", {
				value1: accountName(a, view.tables.model_server),
				value2: deviceName(view, a.device),
			}),
		}));

/**
 * The places a form offers: the workspaces on the devices it picked, then
 * any folder the permission already names.
 */
export const placeChoices = (view: ViewState, draft: ScopeDraft): Choice<AgentPlace>[] => {
	const workspaces: Choice<AgentPlace>[] = all(view, "workspace")
		.filter((w) => draft.devices.includes(w.device))
		.map((w) => ({
			value: { type: "workspace" as const, workspace: w.id },
			label: t("{{value1}} on {{value2}}", {
				value1: workspaceName(w),
				value2: deviceName(view, w.device),
			}),
		}))
		.sort((a, b) => a.label.localeCompare(b.label));
	const folders = (draft.places ?? [])
		.filter((p) => p.type === "folder")
		.map((p) => ({ value: p, label: p.path }));
	return [...workspaces, ...folders];
};

/**
 * The form without the accounts and places it no longer offers, such as
 * an account on a device you unpicked, so a save names only what it shows.
 */
export const offeredOnly = (view: ViewState, draft: ScopeDraft): ScopeDraft => {
	const accountsShown = accountChoices(view, draft);
	const placesShown = placeChoices(view, draft);
	return {
		...draft,
		accounts:
			draft.accounts &&
			draft.accounts.filter((a) => accountsShown.some((c) => c.value === a)),
		places:
			draft.places &&
			draft.places.filter((p) => placesShown.some((c) => samePlace(c.value, p))),
	};
};

/** The limits in a few words, such as "Up to 5 agents and 2 at once". */
export const limitsLine = (limits: Limits | undefined, now: Timestamp): string => {
	const parts: string[] = [];
	if (limits?.max_started != null)
		parts.push(
			t("up to {{plural_limits_max_started___agent}}", {
				plural_limits_max_started___agent: plural(limits.max_started, "agent"),
			})
		);
	if (limits?.max_running != null)
		parts.push(t("{{limits_max_running}} at once", { limits_max_running: limits.max_running }));
	if (limits?.usage_cap_percent != null)
		parts.push(
			t("stops at {{limits_usage_cap_percent}}% usage", {
				limits_usage_cap_percent: limits.usage_cap_percent,
			})
		);
	if (limits?.expires_at != null)
		parts.push(t("ends {{value1}}", { value1: formatWhen(limits.expires_at, now) }));
	if (parts.length === 0) return t("No limits");
	const line = listInWords(parts);
	return line.charAt(0).toUpperCase() + line.slice(1);
};

/** Why a key name won't do as its file's name, or null when it will. */
export const keyNameError = (name: string): string | null => {
	if (name.trim() === "") return t("Give the key a name.");
	if (!/^[A-Za-z0-9_-]+$/.test(name.trim())) {
		return t("Use only letters, digits, dashes and underscores in the name.");
	}
	return null;
};

// Characters a POSIX shell takes as they are.
const PLAIN = /^[\w@%+=:,./-]+$/;

/** One word for a POSIX shell, in single quotes unless it needs none. */
export const shellQuote = (word: string): string =>
	word !== "" && PLAIN.test(word) ? word : `'${word.replaceAll("'", `'\\''`)}'`;

/** The command an MCP client runs for an access key, as one line for a shell. */
export const commandLine = (command: McpCommand): string =>
	[command.command, ...command.args].map(shellQuote).join(" ");
