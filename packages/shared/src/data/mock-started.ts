// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sample agents the release checklist started through the agent tools,
// under the permission it holds, and the calls in its timeline that
// started them and waited on them. One runs on Codex on devbox and one on
// Claude on this Mac, so the sidebar nests both under their caller.

import type { Agent, AgentOrigin, Entity, ItemBody, Timestamp } from "../bindings";

import { CLAUDE, DEVBOX, DEVBOX_CODEX, MAC, MHQ_WORKSPACE } from "./mock-view";

const MIN = 60_000;

/** The agent that holds the sample permission and started the others. */
export const CALLER = "agent-release";
/** The release notes agent it started on devbox. */
export const NOTES = "agent-release-notes";
/** The signed bundle agent it started on this Mac. */
export const BUNDLE = "agent-signed-bundle";

const origin: AgentOrigin = {
	permission: "perm-release",
	by: { kind: "agent", agent: CALLER },
	device: MAC,
	level: 1,
};

const started = (now: Timestamp): Agent[] => [
	{
		id: NOTES,
		workspace: MHQ_WORKSPACE,
		branch: null,
		parent: null,
		vendor: "codex",
		account: DEVBOX_CODEX,
		model: null,
		permission: null,
		session: null,
		vendor_session_id: null,
		control: "held",
		pid: 913,
		link: { state: "live" },
		title: "Write the release notes",
		activity: { state: "done_unread" },
		context: { used_tokens: 22_000, window_tokens: 272_000 },
		started_at: now - 9 * MIN,
		last_activity_at: now - 4 * MIN,
		origin,
	},
	{
		id: BUNDLE,
		workspace: "ws-mac-maxhq",
		branch: null,
		parent: null,
		vendor: "claude",
		account: CLAUDE,
		model: null,
		permission: null,
		session: null,
		vendor_session_id: null,
		control: "held",
		pid: 51_204,
		link: { state: "live" },
		title: "Build a signed bundle",
		activity: { state: "busy" },
		context: { used_tokens: 31_000, window_tokens: 200_000 },
		started_at: now - 8 * MIN,
		last_activity_at: now - 5000,
		origin,
	},
];

/** The started agents, as view entities. */
export const mockStarted = (now: Timestamp): Entity[] =>
	started(now).map((agent) => ({ entity: "agent" as const, ...agent }));

const call = (name: string, title: string, input: object, output: object): ItemBody => ({
	kind: "tool_call",
	server: "maxhq",
	name,
	title,
	input: JSON.stringify(input, null, 2),
	output: JSON.stringify(output, null, 2),
});

/** The release checklist's calls to the agent tools, oldest first. */
export const callerCalls = (): ItemBody[] => [
	call(
		"start_agent",
		"Start an agent",
		{
			request_id: "release-notes",
			task: "Write the release notes for 0.3.0 from the commits since 0.2.0.",
			vendor: "codex",
			device: DEVBOX,
			workspace: MHQ_WORKSPACE,
			title: "Write the release notes",
		},
		{ agent: NOTES, device: DEVBOX }
	),
	call(
		"start_agent",
		"Start an agent",
		{
			request_id: "signed-bundle",
			task: "Build a signed bundle once the version bump lands.",
			vendor: "claude",
			device: MAC,
			title: "Build a signed bundle",
		},
		{ agent: BUNDLE, device: MAC }
	),
	call(
		"wait_agent",
		"Wait for agents",
		{ agents: [NOTES, BUNDLE], timeout_seconds: 60 },
		{ timed_out: true, agents: [] }
	),
];
