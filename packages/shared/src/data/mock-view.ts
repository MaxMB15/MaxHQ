// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sample data shaped like the real setup: this Mac and devbox, a NixOS
// server on the tailnet whose agents run in Incus containers, one Claude Max
// login and one Codex Plus login. Every activity state shows somewhere,
// and so does every place in the inbox: a pinned agent, a snoozed one, one
// back from its snooze, and one settled by you and one by auto-settle.
// The connecting and unreachable dots need devbox to be in trouble, so they
// show through the scenarios at the bottom. Emails are made up.
//
// devbox also has one workspace MaxHQ made, with a setup-token account for
// the Claude Max login and its own Sign in with ChatGPT for the Plus one.
// Neither has usage, since devbox hasn't read any for them.
//
// The Mac also runs Gemini as a local ACP agent, set up in its settings
// with one variable whose value stays on the Mac.

import type {
	Account,
	AcpAgent,
	Agent,
	AttentionEvent,
	Credential,
	Entity,
	ListeningPort,
	Network,
	Project,
	ProjectSettings,
	Timestamp,
	Usage,
	Workspace,
} from "../bindings";
import type { Device, DeviceLink, Entity as FleetEntity } from "../fleet/wire";
import { PROTOCOL_VERSION } from "../bindings";
import { DEFAULT_INBOX } from "../view/inbox";
import { idOf } from "../view/replica";

import type { Scenario } from "./source";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

/** The Mac's device ID. */
export const MAC = "dev-mac";
/** devbox's device ID. */
export const DEVBOX = "dev-devbox";
/** devbox's link ID. */
export const DEVBOX_LINK = "link-devbox";
/** The tailnet's network ID. */
export const TAILNET = "net-tail1234";

/** The next time the clock reads `hour:minute`, today or later in the week on `weekday`. */
const at = (now: Timestamp, hour: number, minute: number, weekday?: number): Timestamp => {
	const date = new Date(now);
	date.setHours(hour, minute, 0, 0);
	if (weekday !== undefined) {
		const ahead = (weekday - date.getDay() + 7) % 7;
		date.setDate(date.getDate() + ahead);
	}
	if (date.getTime() <= now) date.setDate(date.getDate() + (weekday === undefined ? 1 : 7));
	return date.getTime();
};

/** The Incus network container workspaces on devbox join. */
export const DEVBOX_NETWORK = "agentbr0";
/** The Incus storage pool container workspaces on devbox live on. */
export const DEVBOX_POOL = "agents";

/** The Mac's `maxd` build. */
export const MAC_BUILD = "3f9a0c1d2e4b5a67";
/** devbox's `maxd` build, from before the app's last update. */
export const DEVBOX_BUILD = "8b21e07c4d5f6a19";

const devices = (): Device[] => [
	{
		id: MAC,
		name: "MacBook Pro",
		os: "macos",
		boot_id: "mac-boot-1",
		maxd_version: "0.1.0",
		winding_down_until: null,
		workspace_host: null,
		build: MAC_BUILD,
		target: "aarch64-apple-darwin",
	},
	{
		id: DEVBOX,
		name: "devbox",
		os: "linux",
		boot_id: "nl-boot-7",
		maxd_version: "0.1.0",
		winding_down_until: null,
		workspace_host: { networks: [DEVBOX_NETWORK], pools: [DEVBOX_POOL] },
		build: DEVBOX_BUILD,
		target: "x86_64-unknown-linux-musl",
	},
];

const network = (): Network => ({
	id: TAILNET,
	kind: { type: "tailscale", magic_dns_suffix: "tail1234.ts.net", client_version: "1.102.3" },
	name: "Tailnet tail1234",
	state: "up",
});

const links = (now: Timestamp): DeviceLink[] => [
	{
		id: "link-mac",
		name: "Mac",
		device: MAC,
		route: { type: "local" },
		state: { state: "connected", since: now - 3 * HOUR },
		network: null,
		peer: null,
		via: [],
		capabilities: {
			transfers: true,
			tunnels: true,
			workspaces: false,
			inbox: true,
			copy: true,
			load: true,
			updates: true,
			tools: true,
			scheduled_tasks: true,
			acp_agents: true,
			pulls: true,
			github_user: true,
			test_hosts: true,
			workflows: true,
			skills: true,
			host_spec: true,
			tool_installs: true,
			phones: true,
			terminals: true,
		},
		waiting: [],
		routes: [{ route: { type: "local" }, last_good_here: true }],
		protocol: PROTOCOL_VERSION,
	},
	{
		id: DEVBOX_LINK,
		name: "devbox",
		device: DEVBOX,
		route: { type: "ssh", host: "dev@devbox" },
		state: { state: "connected", since: now - 2 * HOUR - 41 * MIN },
		network: TAILNET,
		peer: {
			host_name: "devbox",
			dns_name: "devbox.tail1234.ts.net",
			online: true,
			last_seen: null,
			path: { type: "direct", address: "192.168.1.54:41641" },
			key_expiry: null,
		},
		via: [],
		capabilities: {
			transfers: true,
			tunnels: true,
			workspaces: true,
			inbox: true,
			copy: true,
			load: true,
			updates: true,
			tools: true,
			scheduled_tasks: true,
			pulls: true,
			github_user: true,
			test_hosts: true,
			workflows: true,
			skills: true,
			host_spec: true,
			snapshots: true,
			moves: true,
			tool_installs: true,
			terminals: true,
		},
		waiting: [],
		routes: [
			{ route: { type: "ssh", host: "dev@devbox" }, last_good_here: true },
			{ route: { type: "ssh", host: "dev@devbox.lan" }, last_good_here: false },
		],
		protocol: PROTOCOL_VERSION,
	},
];

const project = (id: string, name: string, repo: string): Project => ({
	id,
	name,
	repo: `github.com/Example-User/${repo}`,
	default_vendor: "claude",
	default_account: null,
	default_workspace: null,
});

const projects = (): Project[] => [
	project("proj-maxhq", "MaxHQ", "MaxHQ"),
	project("proj-comet", "Comet", "Comet"),
	project("proj-dusk-tools", "DuskTools", "DuskTools"),
	project("proj-mosaic", "Mosaic", "Mosaic"),
	project("proj-atlas-maps", "AtlasMaps", "AtlasMaps"),
];

const container = (id: string, projectId: string, instance: string, repo: string): Workspace => ({
	id,
	project: projectId,
	device: DEVBOX,
	location: { type: "container", instance, path: `/home/agent/${repo}` },
});

const workspaces = (): Workspace[] => [
	{
		id: "ws-mac-maxhq",
		project: "proj-maxhq",
		device: MAC,
		location: { type: "folder", path: "/Users/me/Code/Github/Multi/MaxAIManager" },
	},
	{
		id: "ws-mac-comet",
		project: "proj-comet",
		device: MAC,
		location: { type: "folder", path: "/Users/me/Code/Github/Multi/Comet" },
	},
	container("ws-alpha", "proj-atlas-maps", "ws-alpha", "AtlasMaps"),
	container("ws-bravo", "proj-comet", "ws-bravo", "Comet"),
	container("ws-charlie", "proj-dusk-tools", "ws-charlie", "DuskTools"),
	container("ws-echo", "proj-mosaic", "ws-echo", "Mosaic"),
];

/** The workspace MaxHQ made on devbox. */
export const MHQ_WORKSPACE = "ws-mhq-maxhq";

const madeWorkspace = (now: Timestamp): Workspace => ({
	id: MHQ_WORKSPACE,
	project: "proj-maxhq",
	device: DEVBOX,
	location: { type: "container", instance: "mhq-maxhq", path: "/home/agent/work/MaxHQ" },
	managed: {
		state: { state: "running" },
		start_at_boot: true,
		guest: { state: "connected", version: "0.1.0" },
		egress: { state: "passed", at: now - 2 * HOUR - 40 * MIN },
		recipe: 1,
		created_at: now - 3 * DAY,
		unsaved: { changed: 3, commits: 1, no_upstream: 0, read_at: now - 2 * MIN },
	},
});

/** The Claude Max login's ID. */
export const CLAUDE = "acct-claude-mac";
/** The Codex Plus login's ID. */
export const CODEX = "acct-codex-mac";
/** The Mac's Cursor CLI login, which shows no usage meters. */
export const CURSOR = "acct-cursor-mac";
/** The Mac's local ACP agent, and the account its sessions run on. */
export const GEMINI = "acp-gemini-mac";
export const GEMINI_ACCOUNT = "acct-acp-gemini-mac";

/** When the Codex Plus 5-hour window opens again. */
const codexReset = (now: Timestamp) => now + 2 * HOUR + 22 * MIN;

/**
 * The Cursor Pro login's month, as a Mac with Cursor usage on reports it.
 * It shows only once you turn the switch on.
 */
export const cursorMonth = (now: Timestamp): Usage => {
	const cycle = (scope: string | null, used_percent: number) => ({
		scope,
		window_minutes: 0,
		period: "billing_cycle" as const,
		used_percent,
		resets_at: at(now, 0, 0) + 13 * DAY,
	});
	return {
		checked_at: now,
		source: "cursor_dashboard",
		windows: [
			cycle(null, 38),
			cycle("auto", 24),
			cycle("api", 61),
			{ ...cycle("on_demand", 24.8), spend: { used_cents: 1240, limit_cents: 5000 } },
		],
	};
};

/** devbox's account from the Claude Max login's setup-token. */
export const DEVBOX_CLAUDE = "acct-claude-nl-token";
/** devbox's account from its own Sign in with ChatGPT. */
export const DEVBOX_CODEX = "acct-codex-nl-chatgpt";

const accounts = (now: Timestamp): Account[] => [
	{
		id: CLAUDE,
		device: MAC,
		vendor: "claude",
		label: "Claude",
		login: "config_dir",
		origin: null,
		config_dir: "/Users/me/.claude",
		email: "max@example.com",
		organization: null,
		plan: "max",
		models: [],
		usage: {
			checked_at: now - 20_000,
			source: "claude_cache",
			windows: [
				{
					scope: null,
					window_minutes: 300,
					used_percent: 22,
					resets_at: now + 3 * HOUR + 12 * MIN,
				},
				{
					scope: null,
					window_minutes: 10080,
					used_percent: 45,
					resets_at: at(now, 9, 0, 4),
				},
				{
					scope: "opus",
					window_minutes: 10080,
					used_percent: 58,
					resets_at: at(now, 9, 0, 4),
				},
				{
					scope: "fable",
					name: "Fable",
					window_minutes: 10080,
					used_percent: 12,
					resets_at: at(now, 9, 0, 4),
				},
			],
		},
		usage_failure: null,
	},
	{
		id: CODEX,
		device: MAC,
		vendor: "codex",
		label: "Codex",
		login: "config_dir",
		origin: null,
		config_dir: "/Users/me/.codex",
		email: "max@example.net",
		organization: null,
		plan: "plus",
		models: [],
		usage: {
			checked_at: now - 50_000,
			source: "codex_app_server",
			windows: [
				{ scope: null, window_minutes: 300, used_percent: 100, resets_at: codexReset(now) },
				{
					scope: null,
					window_minutes: 10080,
					used_percent: 64,
					resets_at: at(now, 8, 30, 3),
				},
				// Codex Mini frees up first, but the account's own 5 hours
				// still hold it until they reset.
				{
					scope: "codex_mini",
					name: "Codex Mini",
					window_minutes: 300,
					used_percent: 100,
					resets_at: now + HOUR + 5 * MIN,
				},
				{
					scope: "codex_mini",
					name: "Codex Mini",
					window_minutes: 10080,
					used_percent: 31,
					resets_at: at(now, 8, 30, 3),
				},
			],
		},
		usage_failure: null,
	},
	{
		id: CURSOR,
		device: MAC,
		vendor: "cursor",
		label: "Cursor",
		login: "config_dir",
		origin: null,
		config_dir: "/Users/me/.cursor",
		email: "max@example.com",
		organization: null,
		plan: "pro",
		models: [],
		usage: null,
		usage_failure: null,
	},
	{
		id: GEMINI_ACCOUNT,
		device: MAC,
		vendor: "acp",
		label: "Gemini",
		login: "acp_command",
		origin: null,
		config_dir: "",
		email: null,
		organization: null,
		plan: null,
		models: [],
		usage: null,
		usage_failure: null,
		acp: {
			images: true,
			load: true,
			plan: false,
			auth: [
				{ id: "oauth-personal", name: "Log in with Google", description: null },
				{
					id: "gemini-api-key",
					name: "Use Gemini API key",
					description: "Requires setting the GEMINI_API_KEY environment variable",
				},
			],
		},
	},
	{
		id: DEVBOX_CLAUDE,
		device: DEVBOX,
		vendor: "claude",
		label: "Claude",
		login: "setup_token",
		origin: CLAUDE,
		config_dir: `/home/agent/.claude-${DEVBOX_CLAUDE}`,
		email: null,
		organization: null,
		plan: null,
		models: [],
		usage: null,
		usage_failure: null,
	},
	{
		id: DEVBOX_CODEX,
		device: DEVBOX,
		vendor: "codex",
		label: "Codex",
		login: "chat_gpt_sign_in",
		origin: CODEX,
		config_dir: `/home/agent/.codex-${DEVBOX_CODEX}`,
		email: "max@example.net",
		organization: null,
		plan: null,
		models: [],
		usage: null,
		usage_failure: null,
	},
];

// What devbox keeps for its token and sign-in accounts and for the GitHub
// App its workspaces push with. The secrets themselves never leave it.
const credentials = (now: Timestamp): Credential[] => [
	{
		id: "cred-nl-claude",
		device: DEVBOX,
		kind: { type: "claude_token", origin: CLAUDE, account: DEVBOX_CLAUDE, label: "Claude" },
		stored_at: now - 3 * DAY - HOUR,
		problem: null,
	},
	{
		id: "cred-nl-chatgpt",
		device: DEVBOX,
		kind: { type: "chat_gpt", account: DEVBOX_CODEX, email: "max@example.net" },
		stored_at: now - 3 * DAY - 50 * MIN,
		problem: null,
	},
	{
		id: "cred-nl-github",
		device: DEVBOX,
		kind: {
			type: "github_app",
			app_id: 1_000_001,
			slug: "maxhq-workspaces",
			key_sha256: "4f".repeat(32),
			repos: ["Example-User/MaxHQ"],
		},
		stored_at: now - 3 * DAY - 2 * HOUR,
		problem: null,
	},
];

const acpAgents = (): AcpAgent[] => [
	{
		id: GEMINI,
		device: MAC,
		name: "Gemini",
		program: "gemini",
		args: ["--experimental-acp"],
		env: ["GEMINI_API_KEY"],
		containers: false,
		problem: null,
	},
];

const projectSettings = (): ProjectSettings[] => [
	{
		project: "proj-maxhq",
		device: DEVBOX,
		resume_on_restart: true,
		ignored_ports: [],
		git_author: { name: "Max Boksem", email: "max@example.com" },
	},
];

/** What a sample agent sets; the rest takes plain defaults. */
export type AgentSeed = Partial<Agent> & Pick<Agent, "id" | "title" | "activity">;

/** A sample agent, live and observed, from what it sets. */
export const seed = (now: Timestamp, s: AgentSeed & Pick<Agent, "workspace">): Agent => ({
	branch: null,
	parent: null,
	vendor: "claude",
	account: null,
	model: null,
	permission: null,
	session: null,
	vendor_session_id: null,
	control: "observed",
	pid: null,
	link: { state: "live" },
	context: null,
	started_at: now - HOUR,
	last_activity_at: now - 30_000,
	...s,
});

const LEAD = "agent-m1-lead";

const agents = (now: Timestamp): Agent[] => {
	const onMac = (s: AgentSeed): Agent =>
		seed(now, { workspace: "ws-mac-maxhq", account: CLAUDE, pid: 51_000, ...s });
	const sub = (id: string, title: string, activity: Agent["activity"], minutes: number): Agent =>
		onMac({
			id,
			parent: LEAD,
			title,
			activity,
			pid: null,
			started_at: now - minutes * MIN,
			last_activity_at: activity.state === "busy" ? now - 4000 : now - 6 * MIN,
		});
	const onNode = (s: AgentSeed & Pick<Agent, "workspace">): Agent =>
		seed(now, { pid: 812, started_at: now - 5 * HOUR, ...s });

	return [
		// This Mac, MaxHQ, on Claude Max.
		onMac({
			id: LEAD,
			title: "M1 observe milestone",
			activity: { state: "busy_in_subagents" },
			pid: 50_211,
			context: { used_tokens: 118_000, window_tokens: 200_000 },
			started_at: now - 3 * HOUR,
			last_activity_at: now - 72_000,
		}),
		sub("agent-w1", "W1 maxd core", { state: "busy" }, 52),
		sub("agent-w2", "W2 Claude observer", { state: "busy" }, 51),
		sub("agent-w3", "W3 Codex observer", { state: "busy" }, 51),
		sub("agent-w4", "W4 client crate", { state: "idle" }, 50),
		sub("agent-w5", "W5 desktop backend", { state: "busy" }, 50),
		sub("agent-w6", "W6 frontend", { state: "busy" }, 49),
		sub("agent-w7", "W7 Incus observer", { state: "busy" }, 49),
		onMac({
			id: "agent-release",
			title: "Release checklist",
			// MaxHQ runs it, so it can hold the permission to run agents.
			control: "held",
			activity: { state: "needs_input", attention: "att-release" },
			pid: 50_877,
			context: { used_tokens: 41_000, window_tokens: 200_000 },
			last_activity_at: now - 2 * MIN,
		}),
		onMac({
			id: "agent-usage-popover",
			title: "Usage meter popover",
			activity: { state: "done_unread" },
			pid: 50_402,
			context: { used_tokens: 87_000, window_tokens: 200_000 },
			last_activity_at: now - 4 * MIN,
		}),
		onMac({
			id: "agent-open-file",
			title: "Show the release plan",
			activity: { state: "busy" },
			pid: 50_418,
			context: { used_tokens: 12_000, window_tokens: 200_000 },
			last_activity_at: now - 3000,
		}),
		onMac({
			id: "agent-sidebar-sort",
			title: "Sidebar sort order",
			activity: { state: "idle_cache_warm", warm_until: now + 38 * MIN },
			inbox: { ...DEFAULT_INBOX, pin: { at: now - 2 * DAY, key: "a0" } },
			pid: 50_133,
			context: { used_tokens: 63_000, window_tokens: 200_000 },
			last_activity_at: now - 22 * MIN,
		}),
		onMac({
			id: "agent-bindings",
			title: "Bindings check",
			activity: { state: "idle" },
			// Its snooze ended a few minutes ago, so it shows as woke.
			inbox: { ...DEFAULT_INBOX, woke_at: now - 10 * MIN },
			pid: 49_870,
			last_activity_at: now - 75 * MIN,
		}),
		onMac({
			id: "agent-m0-review",
			title: "M0 skeleton review",
			activity: { state: "idle" },
			pid: 38_412,
			started_at: now - 4 * DAY,
			last_activity_at: now - 3 * DAY,
			inbox: { ...DEFAULT_INBOX, settled: { at: now - 2 * DAY, by: "you" } },
		}),
		onMac({
			id: "agent-license-check",
			title: "License allowlist",
			activity: { state: "idle" },
			pid: 40_066,
			started_at: now - 7 * DAY - 3 * HOUR,
			last_activity_at: now - 7 * DAY,
			// Idle past the Mac's five days, so auto-settle settled it.
			inbox: { ...DEFAULT_INBOX, settled: { at: now - 7 * DAY, by: "auto" } },
		}),

		// This Mac, Comet, on Codex Plus.
		seed(now, {
			id: "agent-hdr",
			workspace: "ws-mac-comet",
			vendor: "codex",
			account: CODEX,
			pid: 52_310,
			title: "HDR tone mapping",
			activity: { state: "waiting_for_limit_reset", resets_at: codexReset(now) },
			context: { used_tokens: 151_000, window_tokens: 272_000 },
			last_activity_at: now - 18 * MIN,
		}),
		seed(now, {
			id: "agent-font-fallback",
			workspace: "ws-mac-comet",
			vendor: "codex",
			account: CODEX,
			pid: 52_988,
			title: "Subtitle font fallback",
			activity: { state: "idle_cache_warm", warm_until: now + 3 * MIN },
			context: { used_tokens: 44_000, window_tokens: 272_000 },
			last_activity_at: now - 2 * MIN,
			inbox: { ...DEFAULT_INBOX, snoozed_until: now + 2 * HOUR, snoozed_at: now - MIN },
		}),

		// This Mac, Comet, on Cursor Pro.
		seed(now, {
			id: "agent-cursor-seek",
			workspace: "ws-mac-comet",
			vendor: "cursor",
			account: CURSOR,
			control: "held",
			pid: null,
			title: "Seek bar thumbnails",
			activity: { state: "idle" },
			last_activity_at: now - 25 * MIN,
			// Quiet long enough that the Mac stopped its CLI.
			asleep: true,
		}),

		// devbox, one container per project. Container logins aren't read in M1.
		onNode({
			id: "agent-gtfs",
			workspace: "ws-alpha",
			title: "GTFS feed refresh",
			activity: { state: "busy" },
			context: { used_tokens: 64_000, window_tokens: 200_000 },
			last_activity_at: now - 3000,
		}),
		onNode({
			id: "agent-stop-times",
			workspace: "ws-alpha",
			title: "Stop times cache",
			activity: { state: "paused", reason: "by_you" },
			last_activity_at: now - 40 * MIN,
		}),
		onNode({
			id: "agent-sync-drift",
			workspace: "ws-bravo",
			title: "Subtitle sync drift",
			activity: { state: "needs_input", attention: "att-sync-drift" },
			context: { used_tokens: 182_000, window_tokens: 200_000 },
			last_activity_at: now - 6 * MIN,
		}),
		onNode({
			id: "agent-df-chaos",
			workspace: "ws-charlie",
			title: "df-chaos block",
			activity: { state: "starting" },
			started_at: now - 3 * HOUR,
			last_activity_at: now - 20_000,
		}),
		onNode({
			id: "agent-reboot-bracket",
			workspace: "ws-charlie",
			title: "Workspace reboot bracket",
			activity: { state: "paused", reason: "resumes_made_no_progress" },
			pid: null,
			link: { state: "down", reason: "interrupted by a restart", since: now - 7 * MIN },
			last_activity_at: now - 7 * MIN,
		}),
		onNode({
			id: "agent-egress",
			workspace: "ws-charlie",
			title: "Egress allowlist audit",
			activity: { state: "error", reason: "exited with status 1" },
			pid: null,
			link: { state: "down", reason: "exited", since: now - 12 * MIN },
			last_activity_at: now - 12 * MIN,
		}),
		onNode({
			id: "agent-lesson-audio",
			workspace: "ws-echo",
			title: "Lesson audio pipeline",
			activity: { state: "not_accessible" },
			pid: null,
			link: { state: "unknown" },
			last_activity_at: now - 50 * MIN,
		}),
	];
};

const attention = (now: Timestamp): AttentionEvent[] => [
	{
		id: "att-release",
		agent: "agent-release",
		kind: "permission",
		summary: "Run cargo test --workspace",
		options: [
			{ id: "allow", label: "Allow once" },
			{ id: "session", label: "Allow for this session" },
			{ id: "deny", label: "Deny" },
		],
		accepts_text: false,
		created_at: now - 2 * MIN,
		expires_at: null,
	},
	{
		id: "att-sync-drift",
		agent: "agent-sync-drift",
		kind: "question",
		summary: "Should the sync offset apply per file or per show?",
		options: [
			{ id: "file", label: "Per file" },
			{ id: "show", label: "Per show" },
		],
		accepts_text: true,
		created_at: now - 6 * MIN,
		expires_at: null,
	},
];

// The transit map's dev server, which the GTFS agent started, and a
// database nothing in MaxHQ started, both in the transit container.
const ports = (now: Timestamp): ListeningPort[] => [
	{
		id: "port-transit-vite",
		workspace: "ws-alpha",
		agent: "agent-gtfs",
		port: 5173,
		bind: { type: "any" },
		process: { pid: 2214, name: "node", command: "vite --host" },
		url_hint: "http://localhost:5173/",
		first_seen: now - 4 * MIN,
		gone_since: null,
	},
	{
		id: "port-transit-postgres",
		workspace: "ws-alpha",
		agent: null,
		port: 5432,
		bind: { type: "loopback" },
		process: { pid: 388, name: "postgres", command: "postgres -D /var/lib/postgresql/data" },
		url_hint: null,
		first_seen: now - 5 * HOUR,
		gone_since: null,
	},
];

/** Every entity in the sample view, tagged for a snapshot. */
export const mockEntities = (now: Timestamp): Entity[] => [
	...devices().map((v) => ({ entity: "device" as const, ...v })),
	{ entity: "network" as const, ...network() },
	...links(now).map((v) => ({ entity: "device_link" as const, ...v })),
	...projects().map((v) => ({ entity: "project" as const, ...v })),
	...[...workspaces(), madeWorkspace(now)].map((v) => ({ entity: "workspace" as const, ...v })),
	...accounts(now).map((v) => ({ entity: "account" as const, ...v })),
	...acpAgents().map((v) => ({ entity: "acp_agent" as const, ...v })),
	...agents(now).map((v) => ({ entity: "agent" as const, ...v })),
	...attention(now).map((v) => ({ entity: "attention" as const, ...v })),
	...credentials(now).map((v) => ({ entity: "credential" as const, ...v })),
	...projectSettings().map((v) => ({ entity: "project_settings" as const, ...v })),
	...ports(now).map((v) => ({ entity: "listening_port" as const, ...v })),
];

/** The setups the sample data can switch devbox into. */
export const SCENARIOS: readonly Scenario[] = [
	{ id: "connected", label: "devbox connected" },
	{ id: "rebooting", label: "devbox restarting" },
	{ id: "approval", label: "devbox waiting for this app's key" },
	{ id: "tailscale_off", label: "Tailscale off" },
	{ id: "peer_offline", label: "devbox offline on the tailnet" },
	{ id: "unchecked", label: "devbox not checked yet" },
];

/**
 * Applies a scenario to the base entities. Anything not on devbox is left
 * alone, and while devbox can't be reached its agents read as not
 * accessible.
 */
export const applyScenario = (
	base: readonly FleetEntity[],
	scenario: string,
	now: Timestamp
): FleetEntity[] => {
	if (scenario === "connected") return [...base];
	const nodeWorkspaces = new Set(
		base.filter((e) => e.entity === "workspace" && e.device === DEVBOX).map(idOf)
	);
	return base.map((entity): FleetEntity => {
		if (entity.entity === "agent" && nodeWorkspaces.has(entity.workspace)) {
			return { ...entity, activity: { state: "not_accessible" } };
		}
		if (entity.entity === "network" && entity.id === TAILNET && scenario === "tailscale_off") {
			return { ...entity, state: "off" };
		}
		if (entity.entity === "device_link" && entity.id === DEVBOX_LINK) {
			switch (scenario) {
				case "rebooting":
					return {
						...entity,
						state: { state: "connecting", step: "waiting_for_reboot", attempt: 2 },
					};
				case "approval":
					return {
						...entity,
						state: { state: "connecting", step: "waiting_for_approval", attempt: 1 },
					};
				case "tailscale_off":
					return {
						...entity,
						peer: null,
						state: {
							state: "unreachable",
							reason: "Tailscale is off",
							last_seen: now - 4 * MIN,
						},
					};
				case "peer_offline":
					return {
						...entity,
						peer: entity.peer && {
							...entity.peer,
							online: false,
							last_seen: now - 46 * MIN,
							path: { type: "idle" },
						},
						state: {
							state: "unreachable",
							reason: "devbox is offline on the tailnet",
							last_seen: now - 46 * MIN,
						},
					};
				case "unchecked":
					return { ...entity, peer: null, state: { state: "unknown" } };
			}
		}
		return entity;
	});
};
