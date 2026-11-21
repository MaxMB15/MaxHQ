// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sample agents the release checklist started through the agent tools,
// under the permission it holds, and the calls in its timeline that
// started them and waited on them. One runs on Codex on devbox and one on
// Claude on this Mac, so the sidebar nests both under their caller.
//
// The launch agent on this Mac started six more, one of them through
// another. Most are done, one works and one asks before it pushes a tag, so
// the sidebar folds them under it and keeps the one that needs you in view.

import type { Activity, Agent, AgentOrigin, Entity, ItemBody, Timestamp } from "../bindings";

import { CLAUDE, DEVBOX, DEVBOX_CODEX, MAC, MHQ_WORKSPACE } from "./mock-view";

const MIN = 60_000;

/** The agent that holds the sample permission and started the others. */
export const CALLER = "agent-release";
/** The release notes agent it started on devbox. */
export const NOTES = "agent-release-notes";
/** The signed bundle agent it started on this Mac. */
export const BUNDLE = "agent-signed-bundle";
/** The agent on this Mac that started six agents. */
export const LAUNCH = "agent-launch";
/** The tag agent the launch agent started, which asks before it pushes. */
export const TAG = "agent-launch-tag";
/** The download page agent the launch agent started. */
const DOWNLOADS = "agent-launch-downloads";

const origin: AgentOrigin = {
	permission: "perm-release",
	by: { kind: "agent", agent: CALLER },
	device: MAC,
	level: 1,
};

const byLaunch: AgentOrigin = {
	permission: "perm-launch",
	by: { kind: "agent", agent: LAUNCH },
	device: MAC,
	level: 1,
};

// An agent on this Mac that `by` started `minutes` ago, last active three
// minutes later.
const onMac = (
	now: Timestamp,
	id: string,
	title: string,
	activity: Activity,
	minutes: number,
	by: AgentOrigin | null = byLaunch
): Agent => ({
	id,
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
	pid: 51_300 + minutes,
	link: { state: "live" },
	title,
	activity,
	context: { used_tokens: 14_000, window_tokens: 200_000 },
	started_at: now - minutes * MIN,
	last_activity_at: now - (minutes - 3) * MIN,
	origin: by,
});

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
	onMac(now, LAUNCH, "Prepare the launch", { state: "idle" }, 40, null),
	onMac(now, "agent-launch-post", "Write the launch post", { state: "done_unread" }, 36),
	onMac(now, "agent-launch-shots", "Update the screenshots", { state: "idle" }, 35),
	onMac(now, DOWNLOADS, "Update the download page", { state: "idle" }, 34),
	onMac(now, "agent-launch-links", "Check the download links", { state: "idle" }, 30, {
		...byLaunch,
		by: { kind: "agent", agent: DOWNLOADS },
		level: 2,
	}),
	onMac(now, "agent-launch-demo", "Record the demo", { state: "busy" }, 4),
	onMac(now, TAG, "Tag the release", { state: "needs_input", attention: "att-launch-tag" }, 7),
];

/** The started agents and their callers, as view entities, with the tag agent's prompt. */
export const mockStarted = (now: Timestamp): Entity[] => [
	...started(now).map((agent) => ({ entity: "agent" as const, ...agent })),
	{
		entity: "attention",
		id: "att-launch-tag",
		agent: TAG,
		kind: "permission",
		summary: "Run git push origin v0.3.0",
		options: [
			{ id: "allow", label: "Allow once" },
			{ id: "deny", label: "Deny" },
		],
		accepts_text: false,
		created_at: now - 4 * MIN,
		expires_at: null,
	},
];

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
