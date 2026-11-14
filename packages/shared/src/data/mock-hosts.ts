// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The host entities of the sample data, for the views to start on before
// `maxd` builds them. devbox's host report finds the workspaces' cgroup
// parent missing and zram off, and the Mac runs on the defaults with gh
// missing. The containers you made are each at a different point of their
// move into ones MaxHQ manages: ws-alpha waits for its copy, ws-bravo
// changed since its copy, ws-charlie is kept after the switch, ws-delta's
// set up failed and ws-echo moved back. The Mac's ACP registry lists
// agents in every way they install, and devbox couldn't reach it.

import type { Entity, Move, Timestamp } from "../bindings";
import type { EntityOf } from "../view/replica";
import { DEVBOX, MAC } from "./mock-view";

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

/** The ID of the sample move. */
export const MOCK_MOVE = "move-alpha";

/** What `maxd` puts in a root fix, with the path it runs from. */
export const ROOT_FIX = "sudo /home/dev/.local/bin/maxd host apply --system";

/** A move of a container at `step`, with no plan. */
const moving = (name: string, step: Move["step"], state: Move["state"]): EntityOf<"move"> => ({
	entity: "move",
	id: `move-${name}`,
	device: DEVBOX,
	source: `ws-${name}`,
	target: `mhq-${name}`,
	workspace: null,
	step,
	state,
	plan: null,
	source_changed: false,
	kept_until: null,
});

/** The moves past ws-alpha's, one in each other state a card shows. */
const otherMoves = (now: Timestamp): EntityOf<"move">[] => [
	{ ...moving("bravo", "switch", { state: "ready" }), source_changed: true },
	{ ...moving("charlie", "keep", { state: "ready" }), kept_until: now + 5 * DAY },
	moving("delta", "converge", {
		state: "failed",
		reason: "The guest agent didn't answer within 2 minutes.",
	}),
	moving("echo", "keep", { state: "moved_back" }),
];

/** The host report, the move and the registry, as `maxd` would publish them. */
export const mockHosts = (now: number): Entity[] => [
	{
		entity: "host_report",
		device: DEVBOX,
		source: { type: "file", path: "/etc/maxhq/maxhq-host.toml" },
		checked_at: now - HOUR / 4,
		items: [
			{ id: "incus.pool", label: "Storage pool agents", state: { state: "ok" }, fix: null },
			{ id: "incus.bridge", label: "Bridge agentbr0", state: { state: "ok" }, fix: null },
			{
				id: "caps.parent",
				label: "Workspace cgroup maxhq-workspaces",
				state: { state: "missing" },
				fix: { type: "now" },
			},
			{
				id: "caps.zram",
				label: "Compressed swap",
				state: { state: "drift", want: "on", have: "off" },
				fix: { type: "root_command", command: ROOT_FIX },
			},
			{
				id: "tools.user.node",
				label: "Node.js",
				state: { state: "unknown", why: "Couldn't run node --version." },
				fix: { type: "now" },
			},
		],
	},
	{
		entity: "host_report",
		device: MAC,
		source: { type: "defaults" },
		checked_at: now - HOUR,
		items: [
			{ id: "tools.user.node", label: "Node.js", state: { state: "ok" }, fix: null },
			{
				id: "tools.user.gh",
				label: "GitHub CLI",
				state: { state: "missing" },
				fix: { type: "now" },
			},
		],
	},
	{
		entity: "move",
		id: MOCK_MOVE,
		device: DEVBOX,
		source: "ws-alpha",
		target: "mhq-alpha",
		workspace: "ws-alpha",
		step: "copy",
		state: { state: "ready" },
		plan: {
			look: {
				vendor: "claude",
				session: "sess-alpha",
				agent_state: "idle",
				repo_path: "/home/dev/AtlasMaps",
				profiles: ["default"],
				devices: [],
				disk_used: 6 * 1024 ** 3,
				resume_flag: true,
				next_reboot: null,
			},
			steps: [
				{ step: "copy", changes: ["Copy ws-alpha to mhq-alpha from a new snapshot"] },
				{
					step: "converge",
					changes: ["Install the guest agent", "Set a 3 GiB soft memory limit"],
				},
				{ step: "switch", changes: ["Stop ws-alpha", "Point the workspace at mhq-alpha"] },
				{ step: "keep", changes: ["Keep ws-alpha stopped for 7 days"] },
				{ step: "retire", changes: ["Delete ws-alpha"] },
			],
		},
		source_changed: false,
		kept_until: null,
	},
	{
		entity: "acp_registry",
		device: MAC,
		fetched_at: now - HOUR,
		problem: null,
		agents: [
			{
				id: "gemini",
				name: "Gemini CLI",
				version: "0.12.0",
				description: "Google's agent CLI.",
				install: { type: "npx", package: "@google/gemini-cli" },
				auth: [{ id: "oauth", name: "Sign in with Google", terminal: true }],
				installed: null,
			},
			{
				id: "goose",
				name: "Goose",
				version: "1.9.0",
				description: null,
				install: { type: "binary", checked: true },
				auth: [],
				installed: "1.8.2",
			},
			{
				id: "kiro",
				name: "Kiro",
				version: "0.4.1",
				description: "An agent from a release with no checksum.",
				install: { type: "binary", checked: false },
				auth: [],
				installed: null,
			},
			{
				id: "fast-agent",
				name: "fast-agent",
				version: "0.3.2",
				description: null,
				install: { type: "uvx", package: "fast-agent-acp" },
				auth: [],
				installed: "0.3.2",
			},
			{
				id: "mac-only",
				name: "Desktop agent",
				version: "2.0.0",
				description: "Ships no build for this device.",
				install: null,
				auth: [],
				installed: null,
			},
		],
	},
	{
		entity: "acp_registry",
		device: DEVBOX,
		fetched_at: now - DAY,
		problem: "Couldn't reach the registry. The list is from yesterday.",
		agents: [],
	},
	...otherMoves(now),
];
