// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Small builders for tests. Each fills in plain defaults and takes the
// fields a test cares about.

import type {
	Account,
	Agent,
	AgentProcess,
	AttentionEvent,
	Device,
	DeviceLink,
	Entity,
	Network,
	Project,
	Workspace,
} from "../bindings";

/** A fixed moment, 25 October 2026 at 14:48 local time. */
export const NOW = new Date(2026, 9, 25, 14, 48, 0).getTime();

/** Milliseconds in a minute. */
export const MIN = 60_000;

/** An agent on workspace "w1", idle and live. */
export const agent = (over: Partial<Agent> & { id: string }): Agent => ({
	workspace: "w1",
	branch: null,
	parent: null,
	vendor: "claude",
	account: null,
	model: null,
	permission: null,
	session: null,
	vendor_session_id: null,
	control: "observed",
	pid: 4242,
	link: { state: "live" },
	title: over.id,
	activity: { state: "idle" },
	context: null,
	started_at: NOW - 60 * MIN,
	last_activity_at: NOW - MIN,
	...over,
});

/**
 * A process in an agent's tree, started by the CLI at pid 100 a minute
 * ago, with no ports. Its start is its pid unless a test says otherwise.
 */
export const agentProcess = (over: Partial<AgentProcess> & { pid: number }): AgentProcess => ({
	parent: 100,
	start: over.pid,
	run_ms: MIN,
	cpu_ms: 0,
	memory: 1024 * 1024,
	name: "sh",
	command: "sh",
	cli: false,
	ports: [],
	...over,
});

/** A device link over SSH, connected, to a device that does everything. */
export const link = (over: Partial<DeviceLink> & { id: string }): DeviceLink => ({
	name: over.id,
	device: `dev-${over.id}`,
	route: { type: "ssh", host: `max@${over.id}` },
	state: { state: "connected", since: NOW - 30 * MIN },
	network: null,
	peer: null,
	via: [],
	capabilities: { transfers: true, tunnels: true, workspaces: true },
	waiting: [],
	...over,
});

/** A device running maxd 0.1.0 on Linux. */
export const device = (over: Partial<Device> & { id: string }): Device => ({
	name: over.id,
	os: "linux",
	boot_id: "boot-1",
	maxd_version: "0.1.0",
	winding_down_until: null,
	workspace_host: null,
	...over,
});

/** A tailnet that's up. */
export const network = (over: Partial<Network> = {}): Network => ({
	id: "tailnet",
	kind: { type: "tailscale", magic_dns_suffix: "tail1234.ts.net", client_version: "1.102.3" },
	name: "Tailnet tail1234",
	state: "up",
	...over,
});

/** An account with no usage reading. */
export const account = (over: Partial<Account> & { id: string }): Account => ({
	device: "dev-mac",
	vendor: "claude",
	label: "Claude",
	origin: null,
	config_dir: "/Users/someone/.claude",
	email: "someone@example.com",
	organization: null,
	plan: "max",
	models: [],
	usage: null,
	usage_failure: null,
	...over,
});

/** A project with no defaults. */
export const project = (id: string, name: string): Project => ({
	id,
	name,
	repo: null,
	default_vendor: null,
	default_account: null,
	default_workspace: null,
});

/** A folder workspace. */
export const workspace = (over: Partial<Workspace> & { id: string }): Workspace => ({
	project: null,
	device: "dev-mac",
	location: { type: "folder", path: `/work/${over.id}` },
	...over,
});

/** A permission prompt. */
export const attention = (
	over: Partial<AttentionEvent> & { id: string; agent: string }
): AttentionEvent => ({
	kind: "permission",
	summary: "Run cargo test",
	options: [{ id: "allow", label: "Allow once" }],
	accepts_text: false,
	created_at: NOW - 2 * MIN,
	expires_at: null,
	...over,
});

/** Tags plain records as entities for a snapshot. */
export const entities = (parts: {
	agents?: Agent[];
	links?: DeviceLink[];
	devices?: Device[];
	networks?: Network[];
	accounts?: Account[];
	projects?: Project[];
	workspaces?: Workspace[];
	attention?: AttentionEvent[];
}): Entity[] => [
	...(parts.agents ?? []).map((v) => ({ entity: "agent" as const, ...v })),
	...(parts.links ?? []).map((v) => ({ entity: "device_link" as const, ...v })),
	...(parts.devices ?? []).map((v) => ({ entity: "device" as const, ...v })),
	...(parts.networks ?? []).map((v) => ({ entity: "network" as const, ...v })),
	...(parts.accounts ?? []).map((v) => ({ entity: "account" as const, ...v })),
	...(parts.projects ?? []).map((v) => ({ entity: "project" as const, ...v })),
	...(parts.workspaces ?? []).map((v) => ({ entity: "workspace" as const, ...v })),
	...(parts.attention ?? []).map((v) => ({ entity: "attention" as const, ...v })),
];
