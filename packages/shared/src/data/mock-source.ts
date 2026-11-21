// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sample data as a data source. It keeps the entities in memory, and each
// change goes out as a delta built by comparing what it sent last with what
// it holds now, the same way the backend's replica sends them.

import {
	type AccountId,
	type AcpAgent,
	type AcpAgentId,
	type Agent,
	type AgentId,
	type AgentSpec,
	type AppInfo,
	type AttachmentKind,
	type Change,
	type CloneProgress,
	type CloneState,
	type CommandRule,
	type CopyTarget,
	type Credential,
	type Delta,
	type DeviceId,
	type DeviceLink,
	type Inbox,
	type IgnoredFile,
	type InboxChange,
	type LinkId,
	type LinkState,
	type LoginKind,
	type MoveTo,
	type PermissionMode,
	type SshIncludeState,
	type Timestamp,
	type Vendor,
	type WorkspaceId,
	type WorkspaceSpec,
	type WorkspaceState,
} from "../bindings";
import type { Entity } from "../fleet/wire";
import { apply, inboxOf, isSnoozed } from "../view/inbox";
import { networkReady } from "../settings/network-access";
import { idOf, keyOf as entityKey } from "../view/replica";
import { givenName } from "../ui/rename-agent";
import { runName } from "../workflow/select";

import { mockCliLogins } from "./mock-cli-login";
import { dispatcherEntities, mockDispatcher } from "./mock-dispatcher";
import { mockProcesses } from "./mock-processes";
import { mockPulls } from "./mock-pulls";
import { MOCK_TEST_HOST_SETTINGS, mockTestHosts } from "./mock-test-hosts";
import { isSkillScenario, mockSkills, SKILL_SCENARIOS } from "./mock-skills";
import { mockStarted } from "./mock-started";
import { mockNotes } from "./mock-notes";
import { mockTasks } from "./mock-tasks";
import { createMockTimelines } from "./mock-timeline";
import { mockNodeDetail, mockRunSnapshots, mockWorkflowRuns } from "./mock-workflow";
import {
	applyScenario,
	CURSOR,
	cursorMonth,
	MAC,
	mockEntities,
	DEVBOX_NETWORK,
	DEVBOX,
	DEVBOX_POOL,
	SCENARIOS,
} from "./mock-view";
import type {
	ActionResult,
	CommandResult,
	DataSource,
	MockControls,
	TransferProgress,
} from "./source";

import { findMockGitItems, findMockPaths } from "./mock-find";
import { mockAgents } from "./mock-agents";
import { mockAppKeyEntities, mockAppKeys } from "./mock-app-keys";
import { mockClaims } from "./mock-claims";
import { sampleSessionTerminal } from "./mock-sessions";
import { mockTerminals } from "./mock-terminals";
import { mockFolders } from "./mock-folders";
import { mockNotices } from "./mock-notices";
import { mockLinks } from "./mock-links";
import { mockNetworkAccess, mockPhone, mockPhones } from "./mock-phones";
import { type FleetContext, mockFleet, mockTools } from "./mock-fleet";
import { mockHostCalls } from "./mock-host-calls";
import { mockHosts } from "./mock-hosts";
import { LOADED_MODEL, MAC_SERVER, mockLocalEntities, mockLocalModels } from "./mock-local-models";
import { OUTSIDE_SAMPLE, sampleFile } from "./mock-files";

/** Settings for tests. */
export type MockOptions = {
	/** The clock. Defaults to the real one. */
	readonly now?: () => Timestamp;
	/** Whether busy agents move on their own every few seconds. Defaults to true. */
	readonly tick?: boolean;
	/** How long each step of a sample setup, restart or update takes. Defaults to 700 ms. */
	readonly stepMs?: number;
	/** The setup to start in. */
	readonly scenario?: string;
	/** Where the app's version comes from. Defaults to none, as outside the app. */
	readonly appInfo?: () => Promise<AppInfo | null>;
};

/** The sample source, which always has its extra controls. */
export type MockSource = DataSource & { readonly mock: MockControls };

// How a device's own login for each vendor reaches it.
const LOGIN_OF: Record<Vendor, LoginKind> = {
	claude: "setup_token",
	codex: "chat_gpt_sign_in",
	cursor: "cursor_api_key",
	acp: "acp_command",
};

// Where each vendor keeps its config in containers.
const CONFIG_OF: Record<Vendor, string> = {
	claude: "/home/agent/.claude",
	codex: "/home/agent/.codex",
	cursor: "/home/agent/.cursor",
	acp: "",
};

const TICK_MS = 4000;
const STEP_MS = 700;

const OK: CommandResult = { status: "ok", data: null };

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const done = async () => DONE;

// A random ID shaped like a UUID. React Native has no crypto.randomUUID, and
// sample IDs need no strong randomness.
const randomId = (): string =>
	"xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
		const n = Math.floor(Math.random() * 16);
		return (c === "x" ? n : (n & 3) | 8).toString(16);
	});

/** Stages an attachment the way `maxd` would, so chips in the sample build come out ready. */
const staged = (name: string, mime: string | null, bytes: number): ActionResult => {
	const id = randomId();
	const kind: AttachmentKind =
		mime?.startsWith("image/") === true ? "image" : mime === "application/pdf" ? "pdf" : "file";
	return {
		status: "ok",
		data: {
			status: "staged",
			attachment: {
				id,
				name,
				kind,
				bytes,
				mime,
				path: `/Users/max/.maxhq/attachments/${id}/${name}`,
				thumbnail: null,
			},
		},
	};
};

const MIME_BY_EXTENSION: Record<string, string> = {
	png: "image/png",
	jpg: "image/jpeg",
	jpeg: "image/jpeg",
	gif: "image/gif",
	webp: "image/webp",
	pdf: "application/pdf",
};

/** Where the mock keeps MaxHQ's SSH files. */
const MOCK_SSH = "/Users/someone/Library/Application Support/MaxHQ/ssh";

// The editors the app knows, in its menu's order.
const MOCK_EDITORS = [
	{ id: "cursor", label: "Cursor" },
	{ id: "vscode", label: "VS Code" },
];

/** A container's `Host` entry, the shape the app writes one in. */
const mockHostBlock = (alias: string): string =>
	[
		`Host ${alias}`,
		"  User agent",
		`  ProxyCommand ssh devbox maxd proxy --guest-ssh ${alias}`,
		`  IdentityFile "${MOCK_SSH}/id_ed25519"`,
		"  IdentitiesOnly yes",
		`  UserKnownHostsFile "${MOCK_SSH}/known_hosts"`,
		`  HostKeyAlias ${alias}`,
		"  StrictHostKeyChecking yes",
		"  ForwardAgent no",
		"",
	].join("\n");

/** Guesses a staged path's type from its extension. */
const mimeOf = (path: string): string | null =>
	MIME_BY_EXTENSION[path.split(".").pop()?.toLowerCase() ?? ""] ?? null;

const notFound = (what: string): CommandResult => ({
	status: "error",
	error: { kind: "not_found", what },
});

const missing = (what: string): ActionResult => ({
	status: "error",
	error: { kind: "not_found", what },
});

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

// Each device's settings as its maxd reports them before you change any.
const SETTINGS: Entity[] = [
	{
		entity: "device_settings",
		device: MAC,
		default_permission: "auto",
		workspace_network: null,
		workspace_pool: null,
		auto_settle_days: 5,
		snooze_at_limit: false,
		continue_claude_at_limit: false,
		continue_codex_at_limit: false,
		keep_awake: "charger",
		idle_release_minutes: 60,
		memory_items: 256,
		history_mib: 256,
		claude_setup_token: false,
		shell_integration: true,
		read_other_agents: true,
		dispatcher_acts: true,
		chat_gap_minutes: 30,
		local_models: {
			idle_unload_minutes: 15,
			kv_type: "f16",
			context: 65_536,
			fewer_tools: true,
			linked_devices: true,
			default_model: { server: MAC_SERVER, model: LOADED_MODEL },
		},
		test_hosts: MOCK_TEST_HOST_SETTINGS[MAC],
	},
	{
		entity: "device_settings",
		device: DEVBOX,
		default_permission: "full_access",
		workspace_network: DEVBOX_NETWORK,
		workspace_pool: DEVBOX_POOL,
		auto_settle_days: 5,
		snooze_at_limit: false,
		continue_claude_at_limit: false,
		continue_codex_at_limit: false,
		keep_awake: "charger",
		idle_release_minutes: 60,
		memory_items: 256,
		history_mib: 256,
		// devbox holds a setup-token, so it takes them.
		claude_setup_token: true,
		shell_integration: true,
		read_other_agents: true,
		dispatcher_acts: true,
		chat_gap_minutes: 30,
		local_models: {
			idle_unload_minutes: 15,
			kv_type: "f16",
			context: 65_536,
			fewer_tools: true,
			linked_devices: true,
			default_model: null,
		},
		test_hosts: MOCK_TEST_HOST_SETTINGS[DEVBOX],
	},
	mockNetworkAccess(MAC),
	mockNetworkAccess(DEVBOX),
	{ entity: "hidden_projects", device: MAC, projects: [] },
	{ entity: "hidden_projects", device: DEVBOX, projects: [] },
	{
		entity: "command_rules",
		project: "proj-maxhq",
		device: DEVBOX,
		rules: [
			{ command: "git show", exact: false },
			{ command: "cargo run", exact: false },
			{ command: "rm -rf target/tmp", exact: true },
		],
	},
];

// The steps a new container workspace shows on its way to running.
const CREATE_STEPS = [
	"Making the container",
	"Checking what it can reach",
	"Installing the tools",
	"Cloning the repo",
	"Checking it can push",
];

// What git ignores in a sample checkout: local settings, the certificates
// a dev server uses and a development database.
const IGNORED: IgnoredFile[] = [
	{ path: ".env", size: 1_204 },
	{ path: ".env.local", size: 386 },
	{ path: ".envrc", size: 92 },
	{ path: "apps/api/.dev.vars", size: 171 },
	{ path: "apps/web/.env.development.local", size: 248 },
	{ path: "db/dev.sqlite3", size: 18_874_368 },
	{ path: "db/seed.sql", size: 186_413_056 },
	{ path: "localhost-key.pem", size: 1_704 },
	{ path: "localhost.pem", size: 1_517 },
];

const IGNORED_SIZES = new Map(IGNORED.map((file) => [file.path, file.size]));

// The steps a folder's clone shows, named the way git's progress names them.
const CLONE_START: CloneState = {
	state: "cloning",
	stage: "counting",
	percent: null,
	stalled: false,
};
const CLONE_STEPS: CloneState[] = [
	...[18, 47, 83].map((percent): CloneState => ({
		state: "cloning",
		stage: "receiving",
		percent,
		stalled: false,
	})),
	{ state: "cloning", stage: "resolving", percent: 64, stalled: false },
];

/** Where downloads land on this Mac. */
const DOWNLOADS = "/Users/me/Downloads/MaxHQ";

const keyOf = (e: Entity) => `${e.entity}:${idOf(e)}`;

// The fleet's entities go out as they are. The generated types don't have
// them yet, so they pass as the entities those types know.
const sendable = (e: Entity) => e as Change extends { value: infer V } ? V : never;

const nameOf = (path: string) => path.split("/").pop() ?? path;

const escapeXml = (text: string) =>
	text.replace(/[&<>"]/g, (c) => `&${{ "&": "amp", "<": "lt", ">": "gt", '"': "quot" }[c]};`);

/** A picture that names a sample file, standing in for its bytes. */
const samplePicture = (name: string): string =>
	`data:image/svg+xml,${encodeURIComponent(
		`<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360"><rect width="640" height="360" fill="#262b35"/><text x="320" y="186" fill="#b8bfcc" font-family="sans-serif" font-size="20" text-anchor="middle">${escapeXml(name)}</text></svg>`
	)}`;

/** How big a sample video says it is. */
const SAMPLE_VIDEO_BYTES = 48 * 1024 * 1024;

const VIDEO = /\.(m4v|mov|mp4|ogv|webm)$/i;

const BUSY = new Set(["busy", "busy_in_subagents"]);

// What maxd counts as working when it's asked to settle an agent.
const WORKING = new Set(["busy", "busy_in_subagents", "starting"]);

// The samples whose timeline has a message queued behind what they're doing.
const QUEUED = new Set<AgentId>(["agent-release", "agent-hdr"]);

const SUB_AGENT =
	"only lead agents have a place in the inbox. Settle, snooze or pin the agent that started it";

const OUTSIDE = "this agent runs outside MaxHQ. Continue it in MaxHQ first";

/** A data source over the sample view, with every activity and connection state in it. */
export const mockSource = (options: MockOptions = {}): MockSource => {
	const now = options.now ?? Date.now;
	const stepMs = options.stepMs ?? STEP_MS;
	let base: Entity[] = [
		...mockEntities(now()),
		...mockLocalEntities(now()),
		...mockStarted(now()),
		...SETTINGS,
		mockPhone(now()),
		...mockAppKeyEntities(now()),
		...mockTools(),
		...mockHosts(now()),
		...mockWorkflowRuns(now()),
		...dispatcherEntities(now()),
		{ entity: "terminal", ...sampleSessionTerminal(now()) },
	];
	// The sample tasks publish each device's list as it changes.
	const tasks = mockTasks({
		now,
		changed: () => {
			base = [...base.filter((e) => e.entity !== "device_tasks"), ...tasks.entities()];
			commit();
		},
	});
	base = [...base, ...tasks.entities()];
	// The sample project notes publish each device's copy as it changes.
	const notes = mockNotes({
		now,
		changed: () => {
			base = [...base.filter((e) => e.entity !== "project_notes"), ...notes.entities()];
			commit();
		},
	});
	base = [...base, ...notes.entities()];
	// The sample PRs, with each agent's links on it the way its device
	// publishes them. A sign-in holds a `github_user` login once its code
	// is "entered".
	const pulls = mockPulls({
		now,
		changed: () => {
			base = withPulls(base);
			commit();
		},
		repoOf: (agent) => repoOf(agent),
		login: () => {
			for (const e of base) {
				if (e.entity === "credential" && e.kind.type === "github_user") return e.kind.login;
			}
			return null;
		},
		signedIn: (device, login) =>
			hold({
				id: `cred-github-user-${device}`,
				device,
				kind: { type: "github_user", login },
				stored_at: now(),
				problem: null,
			}),
	});
	const withPulls = (entities: Entity[]): Entity[] =>
		entities.map((e) => {
			const held = e.entity === "agent" ? pulls.linksOf(e.id) : undefined;
			return held ? { ...e, pulls: held } : e;
		});
	base = withPulls(base);
	// The sample test hosts and leases, each its own entity.
	const testHosts = mockTestHosts({
		now,
		changed: () => {
			base = [
				...base.filter((e) => e.entity !== "test_host" && e.entity !== "test_lease"),
				...testHosts.entities(),
			];
			commit();
		},
	});
	base = [...base, ...testHosts.entities()];
	let scenario = options.scenario ?? "connected";
	let scenarioAt = now();
	// The sample skills, whose scenarios touch only them.
	const skills = mockSkills({ now, scenario: () => scenario, changed: () => commit() });
	const scenarios = [...SCENARIOS, ...SKILL_SCENARIOS];
	const retrying = new Map<LinkId, LinkState>();
	// The agents whose inbox changes wait for a link that's down, the way
	// the app's outbox keeps them until the device takes them.
	const outbox = new Map<LinkId, Set<AgentId>>();
	const listeners = new Set<(delta: Delta) => void>();
	let ticker: ReturnType<typeof setInterval> | null = null;
	const timelines = createMockTimelines({ now, stream: options.tick !== false });
	const runs = mockRunSnapshots(now());

	const build = (): Map<string, Entity> => {
		const fleet = isSkillScenario(scenario) ? "connected" : scenario;
		const entities = [...base, ...skills.entities(scenario)];
		const view = applyScenario(entities, fleet, scenarioAt).map((e): Entity => {
			if (e.entity !== "device_link") return e;
			const state = retrying.get(e.id) ?? e.state;
			// A link that comes back takes what waited for it.
			if (state.state === "connected") outbox.delete(e.id);
			return { ...e, state, waiting: [...(outbox.get(e.id) ?? [])].sort() };
		});
		return new Map(view.map((e) => [keyOf(e), e]));
	};

	let current = build();
	let sent = new Map([...current].map(([key, e]) => [key, JSON.stringify(e)]));
	let seq = 1;

	const commit = () => {
		const next = build();
		const changes: Change[] = [];
		for (const [key, e] of next) {
			if (sent.get(key) !== JSON.stringify(e))
				changes.push({ op: "upsert", value: sendable(e) });
		}
		for (const [key, e] of current) {
			if (!next.has(key)) changes.push({ op: "remove", key: entityKey(e) });
		}
		current = next;
		sent = new Map([...next].map(([key, e]) => [key, JSON.stringify(e)]));
		if (changes.length === 0) return;
		seq += 1;
		const delta: Delta = { seq, changes };
		for (const listener of listeners) listener(delta);
	};

	const update = (change: (e: Entity) => Entity) => {
		base = base.map(change);
		commit();
	};

	const tick = () =>
		update((e) => {
			if (e.entity !== "agent" || !BUSY.has(e.activity.state)) return e;
			const context = e.context && {
				...e.context,
				used_tokens: Math.min(e.context.used_tokens + 600, e.context.window_tokens - 1000),
			};
			return { ...e, last_activity_at: now(), context };
		});

	const startTicking = () => {
		if (options.tick === false || ticker) return;
		ticker = setInterval(tick, TICK_MS);
	};

	const stopTicking = () => {
		if (ticker) clearInterval(ticker);
		ticker = null;
	};

	const has = (entity: Entity["entity"], id: string) => current.has(`${entity}:${id}`);

	// A login that signs in again clears the logged out failure, as the
	// device's next usage check does.
	const cliLogins = mockCliLogins({
		accountOf: (id) => {
			const e = current.get(`account:${id}`);
			return e?.entity === "account" ? e : undefined;
		},
		signedIn: (id) =>
			update((e) =>
				e.entity === "account" && e.id === id ? { ...e, usage_failure: null } : e
			),
	});

	// Your ~/.ssh/config starts out without MaxHQ's, and the app hasn't asked.
	let include: "ask" | "included" | "declined" = "ask";

	/** Where an editor stands with a workspace over SSH. */
	const sshState = (workspace: WorkspaceId): SshIncludeState => {
		const record = current.get(`workspace:${workspace}`);
		if (record?.entity !== "workspace") throw new Error("This workspace isn't there anymore");
		if (record.location.type !== "container") {
			return { type: "unavailable", reason: "Only a container opens over MaxHQ's SSH" };
		}
		const alias = `${record.location.instance}.1a2b3c4d.maxhq`;
		if (include === "included") return { type: "included", alias };
		if (include === "declined") return { type: "declined", alias, block: mockHostBlock(alias) };
		return { type: "ask", alias, line: `Include "${MOCK_SSH}/config"` };
	};

	/** An agent's GitHub repository as "owner/name", through its workspace's project. */
	const repoOf = (agent: AgentId): string | null => {
		const record = current.get(`agent:${agent}`);
		if (record?.entity !== "agent") return null;
		const workspace = current.get(`workspace:${record.workspace}`);
		if (workspace?.entity !== "workspace" || workspace.project === null) return null;
		const project = current.get(`project:${workspace.project}`);
		if (project?.entity !== "project") return null;
		return project.repo?.replace(/^(https?:\/\/)?github\.com\//, "") ?? null;
	};

	const retry = (id: LinkId) => {
		retrying.set(id, { state: "connecting", step: "dialing", attempt: 1 });
		commit();
		setTimeout(() => {
			retrying.set(id, { state: "connecting", step: "handshake", attempt: 1 });
			commit();
		}, stepMs);
		setTimeout(() => {
			retrying.delete(id);
			if (scenario !== "connected") scenarioAt = now();
			commit();
		}, 2 * stepMs);
	};

	// The link to the device an agent runs on.
	const linkOf = (agent: Agent): DeviceLink | undefined => {
		const workspace = current.get(`workspace:${agent.workspace}`);
		const device = workspace?.entity === "workspace" ? workspace.device : null;
		const link = [...current.values()].find(
			(e) => e.entity === "device_link" && e.device === device
		);
		return link?.entity === "device_link" ? link : undefined;
	};

	// Why maxd would refuse the change, in its words, or null when it wouldn't.
	// An undo that settles or snoozes the agent again meets the same rules.
	const inboxRefusal = (agent: Agent, change: InboxChange, at: Timestamp): string | null => {
		if (agent.parent !== null) return SUB_AGENT;
		const link = linkOf(agent);
		if (link && link.capabilities?.inbox !== true) {
			return `${link.name}'s maxd is too old to settle, snooze or pin its agents. Update it first`;
		}
		if (change.change === "snooze" && change.until <= at) {
			return "pick a time in the future to snooze the agent until";
		}
		const before = inboxOf(agent);
		const after = apply(before, change, at);
		const settles =
			change.change === "settle" ||
			(change.change === "restore" && before.settled === null && after.settled !== null);
		const snoozes =
			change.change === "snooze" ||
			(change.change === "restore" &&
				isSnoozed(after, at) &&
				after.snoozed_until !== before.snoozed_until);
		if (!settles && !snoozes) return null;
		const what = settles ? "settle" : "snooze";
		const activities = [
			agent.activity.state,
			...[...current.values()].flatMap((e) =>
				e.entity === "agent" && e.parent === agent.id ? [e.activity.state] : []
			),
		];
		if (activities.includes("needs_input")) {
			return `the agent needs your input. Answer it before you ${what} it`;
		}
		if (settles && activities.some((state) => WORKING.has(state))) {
			return "the agent is still working. Stop it or let it finish before you settle it";
		}
		if (QUEUED.has(agent.id)) {
			return `a message is waiting for the agent. Take it back or let it go before you ${what} it`;
		}
		return null;
	};

	// Changes each agent's inbox the way maxd does, all in one delta. While
	// the agent's device is away, the change shows and waits for it.
	const setInbox = async (agents: AgentId[], change: InboxChange): Promise<ActionResult[]> => {
		const at = now();
		const changed = new Map<AgentId, Inbox>();
		const results = agents.map((id): ActionResult => {
			const agent = current.get(`agent:${id}`);
			if (agent?.entity !== "agent") return missing(`agent ${id}`);
			const reason = inboxRefusal(agent, change, at);
			if (reason !== null) return refused(reason);
			changed.set(id, apply(inboxOf(agent), change, at));
			const link = linkOf(agent);
			if (link && link.state.state !== "connected") {
				outbox.set(link.id, (outbox.get(link.id) ?? new Set()).add(id));
			}
			return DONE;
		});
		if (changed.size > 0) {
			update((e) => {
				const inbox = e.entity === "agent" ? changed.get(e.id) : undefined;
				return inbox ? { ...e, inbox } : e;
			});
		}
		return results;
	};

	// Deletes a lead agent the way maxd does, with its sub-agents and its
	// prompts. One from your own terminal, or a sub-agent, is refused.
	const deleteAgent = async (id: AgentId) => {
		dispatcher.wake(id);
		const agent = current.get(`agent:${id}`);
		if (agent?.entity !== "agent") return missing(`agent ${id}`);
		if (agent.control === "observed") return refused(OUTSIDE);
		if (agent.parent !== null) {
			return refused("this is a sub-agent. Delete the agent that started it");
		}
		base = base.filter(
			(e) =>
				!(e.entity === "agent" && (e.id === id || e.parent === id)) &&
				!(e.entity === "attention" && e.agent === id) &&
				!(e.entity === "dispatcher_chat" && e.agent === id)
		);
		commit();
		return DONE;
	};

	// Names an agent the way maxd does, with a blank name bringing back the
	// title from before.
	const titles = new Map<AgentId, string>();
	const renameAgent = async (id: AgentId, title: string) => {
		const agent = current.get(`agent:${id}`);
		if (agent?.entity !== "agent") return missing(`agent ${id}`);
		const before = titles.get(id) ?? agent.title;
		titles.set(id, before);
		const name = givenName(title);
		update((e) =>
			e.entity === "agent" && e.id === id ? { ...e, title: name === "" ? before : name } : e
		);
		return DONE;
	};

	const defaultMode = (device: DeviceId): PermissionMode => {
		const settings = current.get(`device_settings:${device}`);
		return settings?.entity === "device_settings" ? settings.default_permission : "ask";
	};

	// A held agent in the spec's workspace, or in a folder workspace made
	// for it. It starts, then sits idle, since nothing answers its messages.
	let started = 0;
	const startAgent = async (spec: AgentSpec) => {
		const account = current.get(`account:${spec.account}`);
		if (account?.entity !== "account") return missing(`account ${spec.account}`);
		const added: Entity[] = [];
		started += 1;
		let workspace: string;
		if (spec.place.type === "workspace") {
			if (!has("workspace", spec.place.workspace)) {
				return missing(`workspace ${spec.place.workspace}`);
			}
			workspace = spec.place.workspace;
		} else {
			const { path } = spec.place;
			const known = [...current.values()].find(
				(e) =>
					e.entity === "workspace" &&
					e.device === account.device &&
					e.location.type === "folder" &&
					e.location.path === path
			);
			workspace = known ? idOf(known) : `ws-folder-${started}`;
			if (!known) {
				added.push({
					entity: "workspace",
					id: workspace,
					project: null,
					device: account.device,
					location: { type: "folder", path },
				});
			}
		}
		const id = `agent-new-${started}`;
		const at = now();
		const inContainer = current.get(`workspace:${workspace}`);
		added.push({
			entity: "agent",
			id,
			workspace,
			// maxd names a branch for an agent in a container when you don't.
			branch:
				spec.branch ??
				(inContainer?.entity === "workspace" && inContainer.managed ? `maxhq/${id}` : null),
			parent: null,
			vendor: spec.vendor,
			account: spec.account,
			model: spec.model,
			permission: {
				requested: spec.permission ?? defaultMode(account.device),
				effective: null,
			},
			session: null,
			vendor_session_id: null,
			control: "held",
			pid: null,
			link: { state: "live" },
			title: spec.title ?? spec.first?.text.split("\n")[0]?.slice(0, 60) ?? "New agent",
			activity: { state: "starting" },
			context: null,
			started_at: at,
			last_activity_at: at,
		});
		base = [...base, ...added];
		commit();
		setTimeout(
			() =>
				update((e) =>
					e.entity === "agent" && e.id === id && e.activity.state === "starting"
						? { ...e, activity: { state: "idle" } }
						: e
				),
			stepMs
		);
		const result: ActionResult = { status: "ok", data: { status: "started", agent: id } };
		return result;
	};

	// Moves a held lead agent to another device the way maxd does, through
	// the Mac: a new agent starts there under the same name, the old one
	// stops and settles, and the tasks the Mac keeps that send to it follow.
	const moveAgent = async (
		device: DeviceId,
		id: AgentId,
		from: DeviceId,
		to: MoveTo,
		stop: boolean
	): Promise<ActionResult> => {
		if (device !== MAC) return refused("Only the device that runs the MaxHQ app moves agents.");
		if (from === to.device) return refused("The agent is on that device already.");
		const agent = current.get(`agent:${id}`);
		const workspace = agent?.entity === "agent" && current.get(`workspace:${agent.workspace}`);
		if (agent?.entity !== "agent" || !workspace || workspace.entity !== "workspace") {
			return missing(`agent ${id}`);
		}
		if (workspace.device !== from) return missing(`agent ${id}`);
		if (agent.control !== "held" || agent.parent !== null) {
			return refused("Only an agent MaxHQ runs moves, and a sub-agent moves with its lead.");
		}
		if (agent.moved?.to) return refused("That agent moved already.");
		if (!stop && agent.activity.state === "needs_input") {
			return refused("The agent waits for your answer. Answer it, or stop it and move it.");
		}
		if (!stop && WORKING.has(agent.activity.state)) {
			return refused(
				"The agent is working. Wait for its turn to end, or stop it and move it."
			);
		}
		if (!stop && QUEUED.has(id)) {
			return refused(
				"A message waits for the agent. Take it back or let it go, or stop the agent and move it."
			);
		}
		const account = current.get(`account:${to.account}`);
		if (account?.entity !== "account" || account.device !== to.device) {
			return refused("That account isn't on that device.");
		}
		const result = await startAgent({
			vendor: account.vendor,
			account: account.id,
			place: to.place,
			model: to.model ?? null,
			permission: agent.permission?.requested ?? null,
			title: agent.title,
			first: null,
			branch: null,
		});
		if (result.status !== "ok" || result.data.status !== "started") return result;
		const moved = result.data.agent;
		const at = now();
		// Stopping it drops its prompts.
		base = base.filter((e) => !(e.entity === "attention" && e.agent === id));
		update((e) => {
			if (e.entity !== "agent") return e;
			if (e.id === id) {
				return {
					...e,
					activity: { state: "paused", reason: "by_you" },
					pid: null,
					inbox: apply(inboxOf(e), { change: "settle" }, at),
					moved: { to: { device: to.device, agent: moved } },
				};
			}
			return e.id === moved ? { ...e, moved: { from: { device: from, agent: id } } } : e;
		});
		tasks.retarget(MAC, { device: from, agent: id }, { device: to.device, agent: moved });
		return result;
	};

	// The dispatcher on the Mac, which plans through the same sample calls.
	const dispatcher = mockDispatcher({
		now,
		get: (key) => current.get(key),
		entities: () => current.values(),
		change: (edit) => {
			base = edit(base);
			commit();
		},
		timelines,
		startAgent,
		saveTask: tasks.calls.saveTask,
		moveAgent,
		writeNote: notes.write,
	});

	// Container workspaces, which move through their states a step at a time.
	let made = 0;
	const madeWorkspace = (id: WorkspaceId) => {
		const e = current.get(`workspace:${id}`);
		return e?.entity === "workspace" && e.managed ? e : undefined;
	};
	const setState = (id: WorkspaceId, state: WorkspaceState) =>
		update((e) =>
			e.entity === "workspace" && e.id === id && e.managed
				? { ...e, managed: { ...e.managed, state } }
				: e
		);
	const later = (steps: readonly (() => void)[]) =>
		steps.forEach((step, i) => setTimeout(step, (i + 1) * stepMs));

	const createWorkspace = async (spec: WorkspaceSpec) => {
		const project = current.get(`project:${spec.project}`);
		if (project?.entity !== "project") return missing(`project ${spec.project}`);
		const device = current.get(`device:${spec.device}`);
		if (device?.entity !== "device") return missing(`device ${spec.device}`);
		if (!device.workspace_host) return refused(`${device.name} has no Incus`);
		const instance = `mhq-${spec.name ?? project.name.toLowerCase()}`;
		const taken = [...current.values()].some(
			(e) =>
				e.entity === "workspace" &&
				e.location.type === "container" &&
				e.location.instance === instance
		);
		if (taken) return refused(`${instance} already exists`);
		made += 1;
		const id = `ws-made-${made}`;
		const at = now();
		const repo = project.repo?.split("/").pop() ?? project.name;
		const creating = (done: number): WorkspaceState => ({
			state: "creating",
			step: CREATE_STEPS[done] ?? "",
			done,
			of: CREATE_STEPS.length,
		});
		base = [
			...base,
			{
				entity: "workspace",
				id,
				project: spec.project,
				device: spec.device,
				location: { type: "container", instance, path: `/home/agent/work/${repo}` },
				managed: {
					state: creating(0),
					start_at_boot: spec.start_at_boot,
					guest: { state: "installing" },
					egress: { state: "not_run" },
					recipe: 0,
					created_at: at,
				},
			},
		];
		commit();
		later([
			...CREATE_STEPS.slice(1).map((_, i) => () => setState(id, creating(i + 1))),
			() =>
				update((e) =>
					e.entity === "workspace" && e.id === id && e.managed
						? {
								...e,
								managed: {
									...e.managed,
									state: { state: "running" },
									guest: { state: "connected", version: device.maxd_version },
									egress: { state: "passed", at: now() },
									recipe: 1,
								},
							}
						: e
				),
		]);
		const result: ActionResult = { status: "ok", data: { status: "created", workspace: id } };
		return result;
	};

	const startWorkspace = async (id: WorkspaceId) => {
		if (!madeWorkspace(id)) return missing(`workspace ${id}`);
		setState(id, { state: "starting" });
		later([() => setState(id, { state: "running" })]);
		return DONE;
	};

	const stopWorkspace = async (id: WorkspaceId) => {
		if (!madeWorkspace(id)) return missing(`workspace ${id}`);
		setState(id, { state: "stopping" });
		later([() => setState(id, { state: "stopped" })]);
		return DONE;
	};

	// The sample has no unpushed work, so a delete always goes ahead. Its
	// agents go with it.
	const deleteWorkspace = async (id: WorkspaceId) => {
		if (!madeWorkspace(id)) return missing(`workspace ${id}`);
		setState(id, { state: "deleting" });
		later([
			() => {
				base = base.filter(
					(e) =>
						!(e.entity === "workspace" && e.id === id) &&
						!(e.entity === "agent" && e.workspace === id)
				);
				commit();
			},
		]);
		return DONE;
	};

	// Copies of a project. A folder's clone moves through git's steps one at
	// a time. A cancel or retry starts a new round, and the old round's
	// steps lapse.
	let cloned = 0;
	const rounds = new Map<WorkspaceId, number>();
	const nextRound = (id: WorkspaceId) => {
		const round = (rounds.get(id) ?? 0) + 1;
		rounds.set(id, round);
		return round;
	};
	const setClone = (id: WorkspaceId, clone: CloneProgress | null) =>
		update((e) => (e.entity === "workspace" && e.id === id ? { ...e, clone } : e));
	const runClone = (id: WorkspaceId) => {
		const round = nextRound(id);
		const step = (state: CloneState | null) => () => {
			if (rounds.get(id) === round) setClone(id, state && { state });
		};
		setClone(id, { state: CLONE_START });
		later([...CLONE_STEPS.map((state) => step(state)), step(null)]);
	};
	const cloneOf = (id: WorkspaceId) => {
		const e = current.get(`workspace:${id}`);
		return e?.entity === "workspace" ? e.clone?.state : undefined;
	};

	// Why a device's maxd can't take part in a copy, or null when it can.
	const copyRefusal = (device: DeviceId): string | null => {
		const link = [...current.values()].find(
			(e) => e.entity === "device_link" && e.device === device
		);
		if (link?.entity !== "device_link") return "This device isn't there anymore";
		return link.capabilities?.copy === true
			? null
			: `${link.name}'s maxd is too old to copy projects. Update it first`;
	};

	const startCopy = async (project: string, target: CopyTarget): Promise<WorkspaceId> => {
		if (target.type === "container") {
			const made = await createWorkspace({
				project,
				device: target.device,
				name: target.name,
				start_at_boot: true,
			});
			if (made.status === "ok" && made.data.status === "created") return made.data.workspace;
			throw new Error(
				made.status === "ok" && made.data.status === "refused"
					? made.data.reason
					: "The device didn't make the workspace"
			);
		}
		const path = `${target.parent}/${target.name}`;
		const taken = [...current.values()].some(
			(e) =>
				e.entity === "workspace" &&
				e.device === target.device &&
				e.location.type === "folder" &&
				e.location.path === path
		);
		if (taken) throw new Error(`${path} is there already`);
		cloned += 1;
		const id = `ws-cloned-${cloned}`;
		base = [
			...base,
			{
				entity: "workspace",
				id,
				project,
				device: target.device,
				location: { type: "folder", path },
			},
		];
		runClone(id);
		return id;
	};

	// Waits until a new workspace can take files, as the app does: a
	// container running with its guest up, or a folder whose clone is done.
	const whenReady = async (id: WorkspaceId) => {
		for (;;) {
			const e = current.get(`workspace:${id}`);
			if (e?.entity !== "workspace") {
				throw new Error("The new workspace went away before it was ready");
			}
			const clone = e.clone?.state;
			if (clone?.state === "failed") throw new Error(clone.reason);
			if (clone?.state === "cancelled") throw new Error("The clone was cancelled");
			const state = e.managed?.state;
			if (state?.state === "failed") throw new Error(state.reason);
			if (state && !["creating", "starting", "running"].includes(state.state)) {
				throw new Error("The new workspace stopped before it was ready");
			}
			const up =
				!state || (state.state === "running" && e.managed?.guest.state === "connected");
			if (!clone && up) return;
			await new Promise<void>((resolve) => setTimeout(resolve, stepMs));
		}
	};

	// Every chosen file goes in, since the sample's new checkouts hold none
	// of them yet.
	const copyProject: DataSource["copyProject"] = async (from, target, files, onProgress) => {
		const source = current.get(`workspace:${from}`);
		if (source?.entity !== "workspace") throw new Error("This workspace isn't there anymore");
		if (source.project === null) throw new Error("This workspace has no project to copy");
		const refusal = copyRefusal(source.device) ?? copyRefusal(target.device);
		if (refusal !== null) throw new Error(refusal);
		const workspace = await startCopy(source.project, target);
		await whenReady(workspace);
		for (const path of files) onProgress(finished(nameOf(path), IGNORED_SIZES.get(path) ?? 0));
		return { workspace, written: files.length, failed: [] };
	};

	const cancelClone = async (id: WorkspaceId) => {
		const clone = cloneOf(id);
		if (!clone) return missing(`clone ${id}`);
		if (clone.state !== "cloning") return refused("This clone has stopped already");
		nextRound(id);
		setClone(id, { state: { state: "cancelled" } });
		return DONE;
	};

	const retryClone = async (id: WorkspaceId) => {
		const clone = cloneOf(id);
		if (!clone) return missing(`clone ${id}`);
		if (clone.state === "cloning") return refused("This clone is still running");
		runClone(id);
		return DONE;
	};

	const removeClone = async (id: WorkspaceId) => {
		const clone = cloneOf(id);
		if (!clone) return missing(`clone ${id}`);
		if (clone.state === "cloning") return refused("Cancel the clone before you remove it");
		rounds.delete(id);
		base = base.filter((e) => !(e.entity === "workspace" && e.id === id));
		commit();
		return DONE;
	};

	// Logins and keys a device holds. The sample keeps no secret, the same
	// way a device never shows one.
	const hold = (credential: Credential, account?: Entity) => {
		const replaced = new Set([keyOf({ entity: "credential", ...credential })]);
		if (account) replaced.add(keyOf(account));
		base = [
			...base.filter((e) => !replaced.has(keyOf(e))),
			{ entity: "credential", ...credential },
			...(account ? [account] : []),
		];
		commit();
	};

	const loginAccount = (
		id: AccountId,
		device: DeviceId,
		vendor: Vendor,
		label: string,
		origin: AccountId | null
	): Extract<Entity, { entity: "account" }> => ({
		entity: "account",
		id,
		device,
		vendor,
		label,
		login: LOGIN_OF[vendor],
		origin,
		config_dir:
			vendor === "cursor" || vendor === "acp"
				? CONFIG_OF[vendor]
				: `${CONFIG_OF[vendor]}-${id}`,
		email: null,
		organization: null,
		plan: null,
		models: [],
		usage: null,
		usage_failure: null,
	});

	const storeClaudeToken = async (devices: DeviceId[], origin: AccountId, label: string) => {
		if (!has("account", origin)) return devices.map(() => missing(`account ${origin}`));
		return devices.map((device) => {
			if (!has("device", device)) return missing(`device ${device}`);
			// As maxd does, a device that doesn't take setup-tokens refuses one.
			const settings = current.get(`device_settings:${device}`);
			if (settings?.entity === "device_settings" && settings.claude_setup_token === false) {
				return refused(
					"This device doesn't take Claude setup-tokens. Turn them on in its settings first."
				);
			}
			const account = `acct-token-${device}-${origin}`;
			hold(
				{
					id: `cred-token-${device}-${origin}`,
					device,
					kind: { type: "claude_token", origin, account, label },
					stored_at: now(),
					problem: null,
				},
				loginAccount(account, device, "claude", label, origin)
			);
			return DONE;
		});
	};

	// One key on each device, so a second replaces the first.
	const storeCursorKey = async (devices: DeviceId[], label: string) =>
		devices.map((device) => {
			if (!has("device", device)) return missing(`device ${device}`);
			const account = `acct-cursor-${device}`;
			const name = label.trim() || "API key";
			hold(
				{
					id: `cred-cursor-${device}`,
					device,
					kind: { type: "cursor_key", account, label: name },
					stored_at: now(),
					problem: null,
				},
				loginAccount(account, device, "cursor", name, null)
			);
			return DONE;
		});

	const signInChatGpt = async (device: DeviceId) => {
		if (!has("device", device)) return missing(`device ${device}`);
		const origin = [...current.values()].find(
			(e) => e.entity === "account" && e.vendor === "codex" && e.device === MAC
		);
		const account = `acct-chatgpt-${device}`;
		hold(
			{
				id: `cred-chatgpt-${device}`,
				device,
				kind: { type: "chat_gpt", account, email: null },
				stored_at: now(),
				problem: null,
			},
			loginAccount(account, device, "codex", "Codex", origin ? idOf(origin) : null)
		);
		return DONE;
	};

	// A local ACP agent shows as one account named after it. The sample
	// can't run a program, so a save finds it with nothing beyond the basics
	// unless the agent had more before. Values aren't kept anywhere.
	const setAcpAgent = async (device: DeviceId, agent: AcpAgent) => {
		if (!has("device", device)) return missing(`device ${device}`);
		const account = `acct-${agent.id}`;
		const before = current.get(`account:${account}`);
		const acp = before?.entity === "account" ? before.acp : null;
		base = [
			...base.filter(
				(e) =>
					!(e.entity === "acp_agent" && e.id === agent.id) &&
					!(e.entity === "account" && e.id === account)
			),
			{ entity: "acp_agent", ...agent, device, problem: null },
			{
				...loginAccount(account, device, "acp", agent.name, null),
				acp: acp ?? { images: false, load: false, plan: false, auth: [] },
			},
		];
		commit();
		return DONE;
	};

	const deleteAcpAgent = async (device: DeviceId, id: AcpAgentId) => {
		const held = current.get(`acp_agent:${id}`);
		if (held?.entity !== "acp_agent" || held.device !== device) {
			return missing(`ACP agent ${id}`);
		}
		const account = `acct-${id}`;
		const running = base.some(
			(e) => e.entity === "agent" && e.account === account && e.activity.state !== "error"
		);
		if (running) return refused(`Agents of ${held.name} are still running. Stop them first.`);
		base = base.filter(
			(e) =>
				!(e.entity === "acp_agent" && e.id === id) &&
				!(e.entity === "account" && e.id === account)
		);
		commit();
		return DONE;
	};

	const setGithubApp = async (devices: DeviceId[], appId: number) =>
		devices.map((device) => {
			if (!has("device", device)) return missing(`device ${device}`);
			hold({
				id: `cred-github-${device}`,
				device,
				kind: {
					type: "github_app",
					app_id: appId,
					slug: null,
					key_sha256: "4f".repeat(32),
					repos: [],
				},
				stored_at: now(),
				problem: null,
			});
			return DONE;
		});

	const setApnsKey = async (devices: DeviceId[], keyId: string, teamId: string) =>
		devices.map((device) => {
			if (!has("device", device)) return missing(`device ${device}`);
			hold({
				id: `cred-apns-${device}`,
				device,
				kind: { type: "apns", key_id: keyId.trim(), team_id: teamId.trim() },
				stored_at: now(),
				problem: null,
			});
			return DONE;
		});

	// The sample's GitHub never changes, so a check finds what the App
	// already reaches.
	const checkGithubApp = async (device: DeviceId) => {
		if (!has("device", device)) return missing(`device ${device}`);
		const held = base.some(
			(e) => e.entity === "credential" && e.device === device && e.kind.type === "github_app"
		);
		return held
			? DONE
			: refused("This device doesn't hold a GitHub App, so there's nothing to check");
	};

	// Forgetting a token or sign-in also drops the account it gave.
	const forgetCredential = async (device: DeviceId, id: string) => {
		const held = current.get(`credential:${id}`);
		if (held?.entity !== "credential" || held.device !== device) {
			return missing(`credential ${id}`);
		}
		const account = "account" in held.kind ? held.kind.account : null;
		base = base.filter(
			(e) =>
				!(e.entity === "credential" && e.id === id) &&
				!(e.entity === "account" && e.id === account)
		);
		commit();
		// A device signs out of ChatGPT by revoking the session first.
		return held.kind.type === "chat_gpt"
			? ({ status: "ok", data: { status: "signed_out", revoked: true } } as const)
			: DONE;
	};

	// Transfers finish at once, so each reports its end and nothing before.
	const finished = (file: string, bytes: number): TransferProgress => ({
		transfer: randomId(),
		sent: bytes,
		total: bytes,
		file,
		state: "done",
	});

	let forwards = 0;

	const mock: MockControls = {
		scenarios,
		scenario: () => scenario,
		setScenario: (id) => {
			if (!scenarios.some((s) => s.id === id)) return;
			scenario = id;
			scenarioAt = now();
			commit();
		},
	};

	const fleet: FleetContext = {
		now,
		entities: () => current.values(),
		change: (edit) => {
			base = edit(base);
			commit();
		},
		stepMs,
	};
	// A message you send tells the sessions, so their open cards close.
	const { sealSessions, ...terminalCalls } = mockTerminals(fleet, timelines);
	return {
		kind: "mock",
		mock,
		snapshot: async () => ({ seq, entities: [...current.values()].map(sendable) }),
		subscribe: async (onDelta) => {
			listeners.add(onDelta);
			startTicking();
			return () => {
				listeners.delete(onDelta);
				if (listeners.size === 0) stopTicking();
			};
		},
		onFocusAgent: async () => () => {},
		appInfo: options.appInfo ?? (async () => null),
		markSeen: async (agent) => {
			if (!has("agent", agent)) return notFound(`agent ${agent}`);
			update((e) =>
				e.entity === "agent" && e.id === agent && e.activity.state === "done_unread"
					? { ...e, activity: { state: "idle" } }
					: e
			);
			return OK;
		},
		refreshUsage: async (account) => {
			if (account !== null && !has("account", account)) return notFound(`account ${account}`);
			const at = now();
			update((e) =>
				e.entity === "account" && e.usage && (account === null || e.id === account)
					? { ...e, usage: { ...e.usage, checked_at: at } }
					: e
			);
			return OK;
		},
		refreshLinks: async (link) => {
			if (link !== null && !has("device_link", link)) return notFound(`device ${link}`);
			for (const e of current.values()) {
				if (e.entity !== "device_link" || e.route.type !== "ssh") continue;
				if (link === null || e.id === link) retry(e.id);
			}
			return OK;
		},
		setShown: async () => {},
		openTimeline: async (agent, onUpdate) => timelines.open(agent, onUpdate),
		loadOlder: async (agent, before) => timelines.older(agent, before),
		// Each open starts with the run's snapshot as a reset, as `maxd` sends it.
		openWorkflow: async (run, onUpdate) => {
			const snapshot = runs.get(runName(run));
			if (!snapshot) throw new Error(`No run ${runName(run)}`);
			let open = true;
			void Promise.resolve().then(() => {
				if (open) onUpdate({ kind: "reset", run: snapshot });
			});
			return () => {
				open = false;
			};
		},
		nodeDetail: async (run, node) => {
			const snapshot = runs.get(runName(run));
			const lists = snapshot ? mockNodeDetail(snapshot, node) : [];
			if (lists.length === 0) {
				return { status: "error", error: { kind: "not_found", what: `node ${node}` } };
			}
			return { status: "ok", data: { lists } };
		},
		startAgent,
		sendInput: async (agent, input) => {
			// A message closes the cards of commands you ran before it.
			sealSessions(agent, input);
			return DONE;
		},
		findPaths: async (_agent, query) => findMockPaths(query),
		findGitItems: async (agent, query, repo) =>
			findMockGitItems(repo ?? repoOf(agent), query, now()),
		cancelQueued: done,
		interrupt: done,
		setPermission: done,
		setModel: done,
		setEffort: done,
		stopAgent: done,
		deleteAgent,
		moveAgent,
		renameAgent,
		setInbox,
		continueHere: done,
		takeOver: done,
		excerptAgent: async (agent) => {
			const found = current.get(`agent:${agent}`);
			const title = found?.entity === "agent" ? found.title : agent;
			return { name: "agent.md", text: `# Agent: ${title}\n`, messages: 0, omitted: 0 };
		},
		findInTimeline: async (agent, query, at) => timelines.find(agent, query, at),
		answer: async (event, value) => dispatcher.answer(event, value) ?? DONE,
		stageAttachments: async (_device, paths) =>
			paths.map((path) => staged(path.split("/").pop() ?? path, mimeOf(path), 0)),
		stageFound: async (_agent, paths) =>
			paths.map((path) => staged(path.split("/").pop() ?? path, mimeOf(path), 0)),
		stagePasted: async (_device, name, mime, bytes) => staged(name, mime, bytes.byteLength),
		setDeviceSettings: async (settings) => {
			if (!has("device", settings.device)) return missing(`device ${settings.device}`);
			const entity: Entity = { entity: "device_settings", ...settings };
			base = [...base.filter((e) => keyOf(e) !== keyOf(entity)), entity];
			testHosts.place(settings.device, settings.test_hosts?.kinds ?? []);
			const access = mockNetworkAccess(settings.device, settings.network);
			base = [...base.filter((e) => keyOf(e) !== keyOf(access)), access];
			// The Mac's switch is what shows Cursor's month, as maxd's check would.
			if (settings.device === MAC) {
				const usage = settings.cursor_usage ? cursorMonth(now()) : null;
				base = base.map((e) =>
					e.entity === "account" && e.id === CURSOR ? { ...e, usage } : e
				);
			}
			commit();
			return DONE;
		},
		createWorkspace,
		startWorkspace,
		stopWorkspace,
		deleteWorkspace,
		listIgnoredFiles: async (workspace) => {
			if (!has("workspace", workspace)) throw new Error("This workspace isn't there anymore");
			return { files: IGNORED, truncated: false };
		},
		copyProject,
		cancelClone,
		retryClone,
		removeClone,
		sshInclude: async (workspace) => sshState(workspace),
		setupSshInclude: async (workspace, add) => {
			if (sshState(workspace).type !== "unavailable") include = add ? "included" : "declined";
			return sshState(workspace);
		},
		// Both editors are on this Mac, the way the user has them.
		listEditors: async () => ({ available: MOCK_EDITORS, known: MOCK_EDITORS }),
		openInEditor: async (_editor, workspace) =>
			has("workspace", workspace) ? OK : notFound(`workspace ${workspace}`),
		setProjectSettings: async (settings) => {
			if (!has("device", settings.device)) return missing(`device ${settings.device}`);
			if (!has("project", settings.project)) return missing(`project ${settings.project}`);
			const entity: Entity = { entity: "project_settings", ...settings };
			base = [...base.filter((e) => keyOf(e) !== keyOf(entity)), entity];
			commit();
			return DONE;
		},
		// A hide goes on the Mac's list, and a show takes the project off every list.
		setProjectHidden: async (project, hidden) => {
			if (!has("project", project)) return missing(`project ${project}`);
			update((e) => {
				if (e.entity !== "hidden_projects") return e;
				const rest = e.projects.filter((p) => p !== project);
				if (!hidden) return { ...e, projects: rest };
				return e.device === MAC ? { ...e, projects: [...rest, project] } : e;
			});
			return DONE;
		},
		// The list goes once its last rule does, the way maxd drops it.
		removeCommandRule: async (device, project, rule) => {
			if (!has("device", device)) return missing(`device ${device}`);
			const same = (r: CommandRule) => r.command === rule.command && r.exact === rule.exact;
			base = base.flatMap((e) => {
				if (e.entity !== "command_rules" || e.project !== project || e.device !== device)
					return [e];
				const rules = e.rules.filter((r) => !same(r));
				return rules.length === 0 ? [] : [{ ...e, rules }];
			});
			commit();
			return DONE;
		},
		// The user's own global git identity, as the app reads it on their Mac.
		gitIdentity: async () => ({
			name: "Max Boksem",
			email: "12345678+example-user@users.noreply.github.com",
		}),
		windDown: async (device, windowSecs) => {
			if (!has("device", device)) return missing(`device ${device}`);
			const until = now() + windowSecs * 1000;
			update((e) =>
				e.entity === "device" && e.id === device ? { ...e, winding_down_until: until } : e
			);
			return DONE;
		},
		// The service manager restarts maxd, and the link comes back through
		// the same steps a retry shows.
		restartMaxd: async (link) => {
			if (!has("device_link", link)) return notFound(`device ${link}`);
			retry(link);
			return OK;
		},
		// This Mac's link comes back through the same steps a retry shows.
		startLocalMaxd: async () => {
			for (const e of current.values()) {
				if (e.entity === "device_link" && e.route.type === "local") retry(e.id);
			}
			return OK;
		},
		uninstallLocalMaxd: async () => OK,
		storeClaudeToken,
		storeCursorKey,
		signInChatGpt,
		setGithubApp,
		checkGithubApp,
		setApnsKey,
		forgetCredential,
		setAcpAgent,
		deleteAcpAgent,
		upload: async (_device, _target, paths, onProgress) =>
			paths.map((path) => {
				onProgress(finished(nameOf(path), 0));
				return staged(nameOf(path), mimeOf(path), 0);
			}),
		uploadPasted: async (_device, _target, name, mime, bytes, onProgress) => {
			onProgress(finished(name, bytes.byteLength));
			return staged(name, mime, bytes.byteLength);
		},
		attachFromAgent: async (_device, _target, from, path, onProgress) => {
			if (!has("agent", from))
				return { status: "error", error: { kind: "not_found", what: `agent ${from}` } };
			onProgress(finished(nameOf(path), 0));
			return staged(nameOf(path), mimeOf(path), 0);
		},
		cancelTransfer: async () => {},
		download: async (agent, path, onProgress) => {
			if (!has("agent", agent))
				return { status: "error", error: { kind: "not_found", what: `agent ${agent}` } };
			onProgress(finished(nameOf(path), 0));
			return { status: "ok", data: { path: `${DOWNLOADS}/${nameOf(path)}` } };
		},
		media: async (agent, path, limit, anyway, onProgress) => {
			if (!has("agent", agent))
				return { status: "error", error: { kind: "not_found", what: `agent ${agent}` } };
			if (path === OUTSIDE_SAMPLE && !anyway) {
				return {
					status: "ok",
					data: {
						kind: "refused",
						outside: true,
						message: "the path is outside the agent's folders",
					},
				};
			}
			if (VIDEO.test(path)) {
				if (limit < SAMPLE_VIDEO_BYTES)
					return { status: "ok", data: { kind: "too_large", size: SAMPLE_VIDEO_BYTES } };
				return {
					status: "error",
					error: { kind: "failed", message: "The sample data has no videos to play." },
				};
			}
			const sample = sampleFile(path);
			const url = sample?.url ?? samplePicture(nameOf(path));
			const size = sample?.size ?? url.length;
			if (size > limit) return { status: "ok", data: { kind: "too_large", size } };
			onProgress(finished(nameOf(path), size));
			return { status: "ok", data: { kind: "cached", url, size } };
		},
		openForward: async (port) => {
			const listening = current.get(`listening_port:${port}`);
			if (listening?.entity !== "listening_port") {
				return { status: "error", error: { kind: "not_found", what: `port ${port}` } };
			}
			forwards += 1;
			const forward = `fwd-${forwards}`;
			base = [
				...base,
				{
					entity: "forward",
					id: forward,
					port,
					client: "client-mock",
					local_port: listening.port,
					opened_at: now(),
				},
			];
			commit();
			return { status: "ok", data: { forward, local_port: listening.port } };
		},
		closeForward: async (forward) => {
			base = base.filter((e) => !(e.entity === "forward" && e.id === forward));
			commit();
		},
		openLocalhost: async () => {},
		...mockProcesses(now, (agent) => {
			const record = current.get(`agent:${agent}`);
			return record?.entity === "agent" ? record : undefined;
		}),
		...mockFleet(fleet),
		...mockClaims(fleet),
		...terminalCalls,
		...mockFolders(),
		...mockNotices(now),
		...mockHostCalls(fleet),
		...mockLocalModels(fleet),
		...mockAgents(now),
		...mockLinks(),
		...mockAppKeys(now, {
			keys: () => base.filter((e) => e.entity === "app_key"),
			set: (keys) => {
				base = [...base.filter((e) => e.entity !== "app_key"), ...keys];
				commit();
			},
		}),
		...mockPhones(now, (device) => {
			const access = current.get(`network_access:${device}`);
			return access?.entity === "network_access" && networkReady(access);
		}),
		removePhone: async (phone) => {
			base = base.filter((e) => !(e.entity === "phone" && e.id === phone));
			commit();
			return { status: "ok", data: null };
		},
		setPhoneEverything: async (phone, everything) => {
			if (!base.some((e) => e.entity === "phone" && e.id === phone)) {
				return { status: "error", error: { kind: "not_found", what: `phone ${phone}` } };
			}
			base = base.map((e) =>
				e.entity === "phone" && e.id === phone ? { ...e, everything } : e
			);
			commit();
			return { status: "ok", data: [] };
		},
		// The phone signs in once it's open, so nothing shows here yet.
		renewPhoneCodes: async (phone) =>
			base.some((e) => e.entity === "phone" && e.id === phone)
				? { status: "ok", data: [] }
				: { status: "error", error: { kind: "not_found", what: `phone ${phone}` } },
		revokePhoneSession: async (session) => {
			if (!has("phone_session", session)) return missing(`phone session ${session}`);
			base = base.filter((e) => !(e.entity === "phone_session" && e.id === session));
			commit();
			return DONE;
		},
		// Either answer closes the card, as the device does.
		answerAdmission: async (admission) => {
			if (!has("admission", admission)) return missing("That card");
			base = base.filter((e) => !(e.entity === "admission" && e.id === admission));
			commit();
			return DONE;
		},
		...tasks.calls,
		...notes.calls,
		...dispatcher.calls,
		...pulls.calls,
		...cliLogins.calls,
		...testHosts.calls,
		...skills.calls,
	};
};
