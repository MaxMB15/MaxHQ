// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The rows of the phone's agent tools page, worked out once for the page
// so each row takes only its own words: the agents allowed to run agents,
// and the access keys tools outside MaxHQ use. Each names the permission
// Revoke takes off every device.

import type { AccessKey, Permission, PermissionId, Timestamp } from "@maxhq/shared/bindings";
import { agentGrants, keyGrants } from "@maxhq/shared/agent-tools/grants";
import { commandLine, limitsLine } from "@maxhq/shared/agent-tools/scope";
import { t } from "@maxhq/shared/i18n";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";

/** An agent allowed to run agents. */
export type AgentGrantRow = {
	readonly permission: PermissionId;
	/** The agent's title, or a word that it's gone. */
	readonly title: string;
	/** Where the permission lives and its limits. */
	readonly line: string;
};

/** An access key, and the permission it holds. */
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
