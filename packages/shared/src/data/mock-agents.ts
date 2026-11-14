// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent tools calls of the sample data: permissions to run agents,
// access keys and the defaults a new permission starts with. They live in
// memory, and each device holds the permissions that name it, the way
// `maxd` keeps them.

import type { DeviceId, KeyId, Permission, PermissionId, Timestamp } from "../bindings";
import { DEVBOX, DEVBOX_CODEX, MAC } from "./mock-view";
import {
	AGENT_DEFAULTS,
	NO_LIMITS,
	type AccessKey,
	type ActionResult,
	type AgentDefaults,
	type DataSource,
} from "./source";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

// Where each sample device keeps `maxd`'s link and the keys it makes.
const HOMES: Record<DeviceId, { maxd: string; keys: string }> = {
	[MAC]: {
		maxd: "/Users/dev/.local/bin/maxd",
		keys: "/Users/dev/Library/Application Support/MaxHQ/keys",
	},
	[DEVBOX]: { maxd: "/home/dev/.local/bin/maxd", keys: "/home/dev/.local/share/maxhq/keys" },
};

// A key's file name: letters, digits, dashes and underscores, anything
// else made a dash, as `maxd` names it.
const keyFile = (name: string) => `${name.trim().replace(/[^A-Za-z0-9_-]/g, "-") || "key"}.key`;

/** A key as its device lists it, with the file and command `maxd` gives. */
const accessKey = (
	id: KeyId,
	device: DeviceId,
	name: string,
	permission: PermissionId
): AccessKey => {
	const home = HOMES[device] ?? HOMES[MAC]!;
	const path = `${home.keys}/${keyFile(name)}`;
	return {
		id,
		device,
		name,
		path,
		permission,
		command: { command: home.maxd, args: ["mcp", "--key-file", path] },
	};
};

/** The access keys the sample data starts with: one on devbox and one on the Mac. */
export const SAMPLE_KEYS: readonly AccessKey[] = [
	accessKey("key-slack-bot", DEVBOX, "slack-bot", "perm-slack-bot"),
	accessKey("key-raycast", MAC, "raycast", "perm-raycast"),
];

/** The agent on the Mac that may run agents on both devices and lease the Mac's phones. */
const AGENT_PERMISSION: Permission = {
	id: "perm-release",
	holder: { kind: "agent", agent: "agent-release" },
	home: MAC,
	devices: [MAC, DEVBOX],
	vendors: ["claude", "codex"],
	accounts: null,
	places: null,
	depth: AGENT_DEFAULTS.depth,
	limits: NO_LIMITS,
	test_kinds: ["ios_simulator", "android_emulator"],
};

/** The devbox key, which may run two Codex agents at once there. */
const SLACK_PERMISSION: Permission = {
	id: "perm-slack-bot",
	holder: { kind: "key", key: "key-slack-bot" },
	home: DEVBOX,
	devices: [DEVBOX],
	vendors: ["codex"],
	accounts: [DEVBOX_CODEX],
	places: [{ type: "folder", path: "/home/dev/work" }],
	depth: 1,
	limits: { ...NO_LIMITS, max_running: 2 },
};

/** The Mac key, which may run Claude agents on the Mac until a week from now. */
const raycastPermission = (now: Timestamp): Permission => ({
	id: "perm-raycast",
	holder: { kind: "key", key: "key-raycast" },
	home: MAC,
	devices: [MAC],
	vendors: ["claude"],
	accounts: null,
	places: null,
	depth: AGENT_DEFAULTS.depth,
	limits: { ...NO_LIMITS, expires_at: now + 7 * 24 * 60 * 60 * 1000 },
});

const names = (permission: Permission, device: DeviceId) =>
	permission.home === device || permission.devices.includes(device);

/** The agent tools calls over sample permissions and keys. */
export const mockAgents = (
	now: () => Timestamp
): Pick<
	DataSource,
	| "listPermissions"
	| "grantPermission"
	| "revokePermission"
	| "mintKey"
	| "listKeys"
	| "agentDefaults"
	| "setAgentDefaults"
> => {
	const permissions = new Map<PermissionId, Permission>(
		[AGENT_PERMISSION, SLACK_PERMISSION, raycastPermission(now())].map((p) => [p.id, p])
	);
	let keys = [...SAMPLE_KEYS];
	let defaults: AgentDefaults = AGENT_DEFAULTS;
	let minted = 0;

	return {
		listPermissions: async (device) => ({
			status: "ok",
			data: [...permissions.values()].filter((p) => device === null || names(p, device)),
		}),
		grantPermission: async (permission) => {
			permissions.set(permission.id, permission);
			return DONE;
		},
		revokePermission: async (permission) => {
			if (!permissions.delete(permission)) {
				return {
					status: "error",
					error: { kind: "not_found", what: `permission ${permission}` },
				};
			}
			keys = keys.filter((key) => key.permission !== permission);
			return DONE;
		},
		mintKey: async (device, name, permission) => {
			minted += 1;
			const key = accessKey(`key-${minted}`, device, name, permission.id);
			permissions.set(permission.id, { ...permission, holder: { kind: "key", key: key.id } });
			keys = [...keys, key];
			return {
				status: "ok",
				data: {
					status: "key_minted",
					id: key.id,
					path: key.path,
					permission: permission.id,
					command: key.command,
				},
			};
		},
		listKeys: async (device) => ({
			status: "ok",
			data: keys.filter((key) => device === null || key.device === device),
		}),
		agentDefaults: async () => defaults,
		setAgentDefaults: async (next) => {
			defaults = next;
			return { status: "ok", data: null };
		},
	};
};
