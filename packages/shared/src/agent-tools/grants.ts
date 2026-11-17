// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The permissions the agent tools page lists to review and revoke: each
// access key with the one it holds, and those agents hold, which their
// allow sheet grants.

import type { AccessKey, Agent, Permission } from "../bindings";
import type { ViewState } from "../view/replica";

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
