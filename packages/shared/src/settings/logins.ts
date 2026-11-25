// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the login pages work from, on the Mac and on the phone: the Mac's
// own logins a token is minted for, the command that mints it, the devices
// a login can go to, and the logins and keys each device already holds.

import { t } from "../i18n";

import type { Account, Credential, CredentialKind, DeviceId, Timestamp, Vendor } from "../bindings";
import { takesPushes } from "../push/push";
import { listInWords } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";
import { isLive, workspaceHosts } from "../workspaces/state";

import { loggedOut } from "./cli-sign-in";

/** The Mac's device, the one with the local link. */
export const macDevice = (view: ViewState): DeviceId | null =>
	all(view, "device_link").find((l) => l.route.type === "local")?.device ?? null;

/**
 * The logins of a vendor that a config folder holds, on any device, by
 * device and then name. Sign in again runs the CLI's login for each.
 */
export const folderAccounts = (view: ViewState, vendor: Vendor): Account[] =>
	all(view, "account")
		.filter((a) => a.vendor === vendor && (a.login === undefined || a.login === "config_dir"))
		.sort(
			(a, b) =>
				deviceName(view, a.device).localeCompare(deviceName(view, b.device)) ||
				a.label.localeCompare(b.label) ||
				a.id.localeCompare(b.id)
		);

/** The Mac's own logins for a vendor, the ones a folder on the Mac holds. */
export const macAccounts = (view: ViewState, vendor: Vendor): Account[] => {
	const mac = macDevice(view);
	return all(view, "account")
		.filter(
			(a) =>
				a.device === mac &&
				a.vendor === vendor &&
				(a.login === undefined || a.login === "config_dir")
		)
		.sort((a, b) => a.label.localeCompare(b.label) || a.id.localeCompare(b.id));
};

// The folder the CLI uses when `CLAUDE_CONFIG_DIR` isn't set.
const DEFAULT_CLAUDE_DIR = /^\/(Users|home)\/[^/]+\/\.claude\/?$/;

const quoted = (path: string) =>
	/^[\w@%+=:,./-]+$/.test(path) ? path : `'${path.replaceAll("'", `'\\''`)}'`;

/** The command that mints a setup-token for the login in `configDir`. */
export const setupTokenCommand = (configDir: string): string =>
	DEFAULT_CLAUDE_DIR.test(configDir)
		? "claude setup-token"
		: `CLAUDE_CONFIG_DIR=${quoted(configDir)} claude setup-token`;

/** A device a login can go to. */
export type Target = {
	readonly device: DeviceId;
	readonly name: string;
	/** Whether its `maxd` answers, so it can take the login now. */
	readonly live: boolean;
	/** Why it can't take the login even while it answers, when it can't. */
	readonly blocked?: string;
};

/** Whether the device can take the login now, so it can be ticked. */
export const usable = (target: Target): boolean => target.live && !target.blocked;

const targetsOf = (view: ViewState, now: Timestamp, devices: readonly DeviceId[]): Target[] =>
	devices.map((device) => ({
		device,
		name: deviceName(view, device),
		live: isLive(view, device, now),
	}));

/** The devices that host workspaces, the ones a login goes to. */
export const loginTargets = (view: ViewState, now: Timestamp): Target[] =>
	targetsOf(view, now, workspaceHosts(view));

/**
 * Every device MaxHQ knows, by name. The GitHub App can go to any of them,
 * since `#` in the composer reads GitHub on the device the agent runs on.
 */
export const appTargets = (view: ViewState, now: Timestamp): Target[] =>
	targetsOf(
		view,
		now,
		all(view, "device")
			.map((d) => d.id)
			.sort((a, b) => deviceName(view, a).localeCompare(deviceName(view, b)))
	);

/**
 * The devices whose `maxd` pushes to phones, by name. Only they take the
 * APNs key.
 */
export const pushTargets = (view: ViewState, now: Timestamp): Target[] =>
	appTargets(view, now).filter(
		(t) => linkForDevice(view, t.device)?.capabilities?.phones === true
	);

/**
 * The devices that take the Firebase key: those that push to phones. One
 * whose `maxd` is from before FCM_SINCE says it needs an update first.
 */
export const fcmTargets = (view: ViewState, now: Timestamp): Target[] =>
	pushTargets(view, now).map((target) =>
		takesPushes(view, target.device, "fcm")
			? target
			: {
					...target,
					blocked: t("{{name}}'s maxd is too old for this. Update it first.", {
						name: target.name,
					}),
				}
	);

/**
 * The devices to send to: `device` alone when it's given and among them,
 * else all of them. A page opened for one device's error sends only there.
 */
export const onlyTarget = (targets: readonly Target[], device?: DeviceId): Target[] =>
	device !== undefined && targets.some((t) => t.device === device)
		? targets.filter((t) => t.device === device)
		: [...targets];

/** A credential of one kind, with its kind narrowed. */
export type Held<T extends CredentialKind["type"]> = Credential & {
	readonly kind: Extract<CredentialKind, { type: T }>;
};

/** The credentials of one kind that devices hold, by device and then age. */
export const heldOf = <T extends CredentialKind["type"]>(view: ViewState, type: T): Held<T>[] =>
	all(view, "credential")
		.filter((c): c is Held<T> & { entity: "credential" } => c.kind.type === type)
		.sort(
			(a, b) =>
				deviceName(view, a.device).localeCompare(deviceName(view, b.device)) ||
				a.stored_at - b.stored_at
		);

/**
 * The same list with `device`'s first, when it's given, so a page opened
 * for one device's error shows what that device holds at the top.
 */
export const deviceFirst = <T extends { readonly device: DeviceId }>(
	items: readonly T[],
	device?: DeviceId
): T[] =>
	device === undefined
		? [...items]
		: [
				...items.filter((c) => c.device === device),
				...items.filter((c) => c.device !== device),
			];

/**
 * Pasted text with every space and line break taken out, for a value that
 * never has one. A terminal wraps a long token, so its copy can.
 */
export const withoutWhitespace = (text: string): string => text.replace(/\s+/g, "");

/** The App ID typed in, or null when it isn't a whole number above 0. */
export const parseAppId = (text: string): number | null => {
	const digits = withoutWhitespace(text);
	if (!/^\d{1,12}$/.test(digits)) return null;
	const id = Number(digits);
	return id > 0 ? id : null;
};

/** The file name at the end of a path. */
export const fileName = (path: string): string => path.split(/[\\/]/).pop() ?? path;

/** What a device's GitHub App reaches, in words. */
export const reachText = (repos: readonly string[]): string =>
	repos.length > 0
		? t("Reaches {{listInWords}}.", { listInWords: listInWords(repos) })
		: t("Reaches no repo yet. Install the App on the repos your projects use.");

/** Who a CLI login is signed in as, or that it's signed out. */
export const signedInAs = (account: Account): string => {
	if (loggedOut(account)) return t("Signed out");
	return account.email ? t("Signed in as {{email}}", { email: account.email }) : t("Signed in");
};

/** Where ChatGPT shows how much of the plan's limits you've used. */
export const CODEX_USAGE_URL = "https://chatgpt.com/#settings/Usage";
