// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the login pages work from: the Mac's own logins a token is minted
// for, the command that mints it, the devices a login can go to, and the
// logins and keys each device already holds.

import type {
	Account,
	Credential,
	CredentialKind,
	DeviceId,
	Timestamp,
	Vendor,
} from "@maxhq/shared/bindings";
import { all, type ViewState } from "@maxhq/shared/view/replica";
import { deviceName, linkForDevice } from "@maxhq/shared/view/select";
import { isLive, workspaceHosts } from "@maxhq/shared/workspaces/state";

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
