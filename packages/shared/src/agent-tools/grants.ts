// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The permissions the agent tools page lists to review and revoke: each
// access key with the one it holds, and those agents hold, which their
// allow sheet grants. The rows put them in words once for the page, the
// same on the desktop and the phone, so each row takes only its own.

import { t } from "../i18n";

import type { AccessKey, Agent, Permission, PermissionId, Timestamp } from "../bindings";
import type { ViewState } from "../view/replica";
import { deviceName } from "../view/select";

import { commandLine, limitsLine } from "./scope";

/** An access key and the permission it holds, once the permissions are read. */
export type KeyGrant = { readonly key: AccessKey; readonly permission: Permission | undefined };

/** Each key with the permission it holds, in the order the keys came. */
export const keyGrants = (
	keys: readonly AccessKey[],
	permissions: readonly Permission[]
): KeyGrant[] => {
	const byId = new Map(permissions.map((p) => [p.id, p]));
	return keys.map((key) => ({ key, permission: byId.get(key.permission) }));
};

/** A permission an agent holds, and the agent, or undefined when it's gone. */
export type AgentGrant = { readonly permission: Permission; readonly agent: Agent | undefined };

/**
 * The permissions agents hold, by the agent's title, with those of agents
 * MaxHQ no longer knows last.
 */
export const agentGrants = (view: ViewState, permissions: readonly Permission[]): AgentGrant[] =>
	permissions
		.flatMap((permission) =>
			permission.holder.kind === "agent"
				? [{ permission, agent: view.tables.agent.get(permission.holder.agent) }]
				: []
		)
		.sort((a, b) => {
			if (!a.agent || !b.agent) return a.agent ? -1 : b.agent ? 1 : 0;
			return a.agent.title.localeCompare(b.agent.title);
		});

/** An agent allowed to run agents, as its row says it. */
export type AgentGrantRow = {
	readonly permission: PermissionId;
	/** The agent's title, or a word that it's gone. */
	readonly title: string;
	/** Where the permission lives and its limits. */
	readonly line: string;
};

/** An access key and the permission it holds, as its row says them. */
export type KeyGrantRow = {
	readonly id: string;
	readonly permission: PermissionId;
	readonly name: string;
	/** Where the key's file is and the permission's limits, once read. */
	readonly line: string;
	/** The key's file, which stays on its device. */
	readonly path: string;
	/** The command an MCP client runs for the key, as one line for a shell. */
	readonly command: string;
};

const onDevice = (view: ViewState, device: string): string =>
	t("On {{name}}", { name: deviceName(view, device) });

/** "On devbox. No limits", or only where while the permission isn't read. */
const grantLine = (
	view: ViewState,
	device: string,
	permission: Permission | undefined,
	now: Timestamp
): string =>
	permission
		? `${onDevice(view, device)}. ${limitsLine(permission.limits, now)}`
		: onDevice(view, device);

/** The agents allowed to run agents, by title, with those MaxHQ no longer knows last. */
export const agentRows = (
	view: ViewState,
	permissions: readonly Permission[],
	now: Timestamp
): AgentGrantRow[] =>
	agentGrants(view, permissions).map(({ permission, agent }) => ({
		permission: permission.id,
		title: agent ? agent.title || t("This agent") : t("This agent is gone"),
		line: grantLine(view, permission.home, permission, now),
	}));

/** The access keys, each with its permission's limits once the permissions are read. */
export const keyRows = (
	view: ViewState,
	keys: readonly AccessKey[],
	permissions: readonly Permission[],
	now: Timestamp
): KeyGrantRow[] =>
	keyGrants(keys, permissions).map(({ key, permission }) => ({
		id: key.id,
		permission: key.permission,
		name: key.name,
		line: grantLine(view, key.device, permission, now),
		path: key.path,
		command: commandLine(key.command),
	}));
