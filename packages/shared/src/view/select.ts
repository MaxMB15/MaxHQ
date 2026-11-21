// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Read-only questions over the view: which agents sit under which project,
// in what order, which are folded away, which sit pinned, snoozed or
// settled, and how devices group under their networks (N-5). Nothing here
// changes the view.

import { t } from "../i18n";

import type {
	Account,
	Actor,
	Agent,
	AttentionEvent,
	DeviceLink,
	ModelChoice,
	Network,
	Project,
	Timestamp,
	Vendor,
	Workspace,
} from "../bindings";
import {
	agentConnection,
	deviceConnection,
	type ConnectionLook,
	type LinkContext,
	worstLook,
} from "../status/connection";
import { atLimit } from "../status/usage";
import { claimNames } from "../ui/link-claim";
import { liveRunAgents } from "../workflow/select";

import { inboxOf, isSnoozed } from "./inbox";
import { projectsWithNotes } from "./note-heads";
import { byPin } from "./pin-order";
import { all, type EntityOf, type ViewState } from "./replica";

/** Idle agents older than this fold into the "older" row (U-8). */
export const FOLD_AFTER_MS = 24 * 60 * 60 * 1000;

const WORKING = new Set(["busy", "busy_in_subagents", "starting"]);

/**
 * Where an activity sorts within its project: needs input first, then
 * working, then the agents that came back to you, done and unread or woken
 * from a snooze, then everything that waits, with idle last. An agent that
 * stops running so drops below the ones still at it.
 */
export const sortRank = (agent: Agent): number => {
	if (agent.activity.state === "needs_input") return 0;
	if (WORKING.has(agent.activity.state)) return 1;
	if (inboxOf(agent).woke_at !== null) return 2;
	switch (agent.activity.state) {
		case "done_unread":
			return 2;
		case "waiting_for_limit_reset":
			return 3;
		case "error":
			return 4;
		case "paused":
			return 5;
		case "not_accessible":
			return 6;
		default:
			return 7;
	}
};

/**
 * What agents of one rank sort by. Running agents keep the order they
 * started in, since each command they run would otherwise swap them.
 */
const sortTime = (agent: Agent): Timestamp =>
	WORKING.has(agent.activity.state) ? agent.started_at : agent.last_activity_at;

/** Sorts agents by rank, then the latest first. */
export const byRank = (a: Agent, b: Agent): number =>
	sortRank(a) - sortRank(b) || sortTime(b) - sortTime(a) || a.id.localeCompare(b.id);

/**
 * Whether the agent folds into the "older" row. Only idle agents fold, and
 * only once they've been idle for a day. A done-unread agent never folds
 * before you open it, nor does one just woken from a snooze, nor anything
 * that's working or waiting on you.
 */
export const isFolded = (agent: Agent, now: Timestamp): boolean =>
	(agent.activity.state === "idle" || agent.activity.state === "idle_cache_warm") &&
	inboxOf(agent).woke_at === null &&
	now - agent.last_activity_at > FOLD_AFTER_MS;

/** Where a top-level agent sits in the projects view. */
export type InboxPlace = "pinned" | "active" | "working" | "snoozed" | "settled";

/** What the projects view lists, as the app's own settings say. */
export type SectionOptions = {
	/** Whether busy agents move to the Working shelf at the bottom. */
	readonly workingShelf: boolean;
};

/**
 * Where an agent sits. A snooze outranks a settle, a settle outranks a pin,
 * and a pin outranks the Working shelf, so each agent shows once.
 */
export const placeOf = (agent: Agent, now: Timestamp, options: SectionOptions): InboxPlace => {
	const inbox = inboxOf(agent);
	if (isSnoozed(inbox, now)) return "snoozed";
	if (inbox.settled !== null) return "settled";
	if (inbox.pin !== null) return "pinned";
	if (options.workingShelf && WORKING.has(agent.activity.state)) return "working";
	return "active";
};

const NO_SHELF: SectionOptions = { workingShelf: false };

/**
 * The view's link to the device with this ID, if the client has one: a
 * connected one first. A link that waits for your answer about which device
 * it is never counts.
 */
export const linkForDevice = (view: ViewState, device: string): DeviceLink | undefined => {
	const links = all(view, "device_link").filter((l) => l.device === device && !l.claim);
	return links.find((l) => l.state.state === "connected") ?? links[0];
};

/** The name a device goes by in the sidebar: your name for its link, or its own. */
export const deviceName = (view: ViewState, device: string): string =>
	linkForDevice(view, device)?.name ??
	view.tables.device.get(device)?.name ??
	t("Unknown device");

/** Everything the connection helpers need for one link. */
export const linkContext = (view: ViewState, link: DeviceLink): LinkContext => {
	const links = all(view, "device_link");
	const jumpHosts = link.via
		.map((host) =>
			links.find((l) => l.name === host || (l.route.type === "ssh" && l.route.host === host))
		)
		.filter((l): l is EntityOf<"device_link"> => l !== undefined && l.id !== link.id);
	const deviceId = link.device;
	const agents =
		deviceId === null
			? []
			: all(view, "agent").filter(
					(a) => view.tables.workspace.get(a.workspace)?.device === deviceId
				);
	return {
		link,
		device: deviceId === null ? undefined : view.tables.device.get(deviceId),
		network: link.network === null ? undefined : view.tables.network.get(link.network),
		jumpHosts,
		agents,
		claimed: claimNames(view, link) ?? undefined,
	};
};

/** The link context for the device an agent runs on. */
export const agentLinkContext = (view: ViewState, agent: Agent): LinkContext | undefined => {
	const workspace = view.tables.workspace.get(agent.workspace);
	if (!workspace) return undefined;
	const link = linkForDevice(view, workspace.device);
	return link ? linkContext(view, link) : undefined;
};

/** The link an inbox change for the agent waits to go out through, if one waits. */
export const waitingFor = (view: ViewState, agent: string): DeviceLink | undefined =>
	all(view, "device_link").find((l) => l.waiting.includes(agent));

/** The agent's sub-agents, oldest first. */
export const childrenOf = (view: ViewState, agent: string): Agent[] =>
	all(view, "agent")
		.filter((a) => a.parent === agent)
		.sort((a, b) => a.started_at - b.started_at || a.id.localeCompare(b.id));

/**
 * The agent that started this one through the agent tools, when the
 * sidebar shows it under that agent: the caller is in the view and is a
 * row itself, and you haven't pinned, snoozed or settled the started one.
 * Otherwise it stays in the lists, so it's never out of reach.
 */
export const callerOf = (view: ViewState, agent: Agent, now: Timestamp): Agent | undefined => {
	const by = agent.origin?.by;
	if (by?.kind !== "agent" || agent.parent !== null) return undefined;
	const caller = view.tables.agent.get(by.agent);
	if (caller === undefined || caller.parent !== null || caller.id === agent.id) return undefined;
	return placeOf(agent, now, NO_SHELF) === "active" ? caller : undefined;
};

/** The agents shown under this one because it started them, oldest first. */
export const startedBy = (view: ViewState, agent: string, now: Timestamp): Agent[] =>
	all(view, "agent")
		.filter((a) => callerOf(view, a, now)?.id === agent)
		.sort((a, b) => a.started_at - b.started_at || a.id.localeCompare(b.id));

/** One row under a caller, and how many levels below it the row sits. */
export type StartedRow = { readonly agent: Agent; readonly level: number };

/**
 * Every agent shown under this one, in the order the rows run: each agent
 * it started, then the ones that agent started, and so on down.
 */
export const startedRows = (view: ViewState, agent: string, now: Timestamp): StartedRow[] => {
	const rows: StartedRow[] = [];
	const seen = new Set([agent]);
	const walk = (caller: string, level: number) => {
		for (const child of startedBy(view, caller, now)) {
			if (seen.has(child.id)) continue;
			seen.add(child.id);
			rows.push({ agent: child, level });
			walk(child.id, level + 1);
		}
	};
	walk(agent, 1);
	return rows;
};

/** Who acted, in words: an agent's title, "an access key" or "the task Nightly". */
export const actorName = (view: ViewState, actor: Actor): string => {
	if (actor.kind === "agent") return view.tables.agent.get(actor.agent)?.title ?? t("an agent");
	if (actor.kind === "key") return t("an access key");
	const task = all(view, "device_tasks")
		.flatMap((d) => d.tasks)
		.find((t) => t.id === actor.task);
	return task ? t("the task {{task_name}}", { task_name: task.name }) : t("a scheduled task");
};

/** "Started by Release checklist on Mac", for an agent the agent tools started. */
export const startedByLine = (view: ViewState, agent: Agent): string | undefined => {
	const origin = agent.origin;
	if (!origin) return undefined;
	return t("Started by {{actorName_view__origin_by}} on {{deviceName_view__origin_device}}", {
		actorName_view__origin_by: actorName(view, origin.by),
		deviceName_view__origin_device: deviceName(view, origin.device),
	});
};

/** The agent's open prompt, while it needs input. */
export const attentionOf = (view: ViewState, agent: Agent): AttentionEvent | undefined =>
	agent.activity.state === "needs_input"
		? view.tables.attention.get(agent.activity.attention)
		: undefined;

/** The projects hidden from the sidebar: each one any device's list names. */
export const hiddenProjects = (view: ViewState): ReadonlySet<string> =>
	new Set(all(view, "hidden_projects").flatMap((list) => list.projects));

/** Whether a device's `maxd` keeps hidden projects, so a project can be hidden. */
export const canHideProjects = (view: ViewState): boolean => view.tables.hidden_projects.size > 0;

/** The hidden projects the view has, by name, for the Hidden projects entry. */
export const hiddenProjectList = (view: ViewState): Project[] =>
	[...hiddenProjects(view)]
		.flatMap((id) => {
			const project = view.tables.project.get(id);
			return project ? [project] : [];
		})
		.sort(
			(a, b) =>
				a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) ||
				a.id.localeCompare(b.id)
		);

const CALLS_BACK = new Set<Agent["activity"]["state"]>([
	"starting",
	"busy",
	"busy_in_subagents",
	"waiting_for_limit_reset",
	"needs_input",
	"done_unread",
	"error",
]);

/**
 * Whether the agent brings its hidden project back: it's working or waiting
 * for its limit, needs you, is done and unread, woke from a snooze or hit
 * an error.
 */
export const callsBack = (agent: Agent): boolean =>
	CALLS_BACK.has(agent.activity.state) || inboxOf(agent).woke_at !== null;

/**
 * The workspaces the devices' dispatchers run in. Their agents live in the
 * header, so the sidebar leaves them out, but Needs you keeps them.
 */
export const dispatcherHomes = (view: ViewState): ReadonlySet<string> =>
	new Set(all(view, "dispatcher").map((d) => d.workspace));

/** One project in the sidebar. */
export type ProjectGroup = {
	/** The project's ID, or "none" for agents in workspaces without one. */
	readonly key: string;
	/** The project, unless this is the group for agents without one. */
	readonly project?: Project;
	/** Whether you hid the project, so it shows only while an agent calls it back. */
	readonly hidden: boolean;
	/** The header text. */
	readonly name: string;
	/** The devices the shown agents run on, for the header's right side. */
	readonly devices: readonly string[];
	/** Top-level agents shown as rows, sorted. */
	readonly agents: readonly Agent[];
	/** Idle agents folded into the "older" row, most recent first. */
	readonly older: readonly Agent[];
};

const NO_PROJECT = "none";

/** The names of the devices the agents run on, each once, in the agents' order. */
export const devicesOf = (view: ViewState, agents: readonly Agent[]): string[] => [
	...new Set(
		agents.map((a) => {
			const ws = view.tables.workspace.get(a.workspace);
			return ws ? deviceName(view, ws.device) : t("Unknown device");
		})
	),
];

/**
 * Top-level agents in the active list, grouped by project, each group
 * sorted, with idle agents over a day old folded away. Pinned, snoozed and
 * settled agents sit in their own sections, and busy ones too while the
 * Working shelf is on. A project with live notes or a live workflow run
 * gets a group even with no active agent, so its Notes or Workflows entry
 * shows. The agents a live run's nodes are bound to stay out, since the
 * Workflows entry shows the run. A hidden project shows only while
 * one of its agents, or one shown under them, calls it back. Projects sort
 * by name, and agents without one come last. A dispatcher's agents aren't
 * listed.
 */
export const projectGroups = (
	view: ViewState,
	now: Timestamp,
	options: SectionOptions = NO_SHELF
): ProjectGroup[] => {
	const buckets = new Map<string, Agent[]>();
	const homes = dispatcherHomes(view);
	const inRuns = liveRunAgents(view);
	for (const agent of all(view, "agent")) {
		if (agent.parent !== null || placeOf(agent, now, options) !== "active") continue;
		if (callerOf(view, agent, now) || homes.has(agent.workspace)) continue;
		if (inRuns.has(agent.id)) continue;
		const workspace = view.tables.workspace.get(agent.workspace);
		const key = workspace?.project ?? NO_PROJECT;
		const bucket = buckets.get(key) ?? [];
		bucket.push(agent);
		buckets.set(key, bucket);
	}
	const kept = [
		...projectsWithNotes(view),
		...all(view, "workflow_run").flatMap((run) => (run.done ? [] : [run.key.project])),
	];
	for (const project of kept) {
		if (!buckets.has(project) && view.tables.project.has(project)) buckets.set(project, []);
	}

	const hidden = hiddenProjects(view);
	const shown = (agent: Agent) =>
		callsBack(agent) || startedRows(view, agent.id, now).some((row) => callsBack(row.agent));
	const groups = [...buckets].flatMap(([key, agents]): ProjectGroup[] => {
		if (hidden.has(key) && !agents.some(shown)) return [];
		const project = key === NO_PROJECT ? undefined : view.tables.project.get(key);
		const sorted = agents.sort(byRank);
		return [
			{
				key,
				project,
				hidden: hidden.has(key),
				name: project?.name ?? t("No project"),
				devices: devicesOf(view, sorted),
				agents: sorted.filter((a) => !isFolded(a, now)),
				older: sorted.filter((a) => isFolded(a, now)),
			},
		];
	});

	return groups.sort((a, b) => {
		if (a.key === NO_PROJECT) return 1;
		if (b.key === NO_PROJECT) return -1;
		return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
	});
};

/** A project group's agents on one device. */
export type DeviceAgents = {
	/** The device's ID, or "none" when the workspace is unknown. */
	readonly key: string;
	/** The device's name. */
	readonly name: string;
	/** The agents, in the order they came. */
	readonly agents: readonly Agent[];
};

const NO_DEVICE = "none";

/** Agents split by the device they run on, devices by name. */
export const byDevice = (view: ViewState, agents: readonly Agent[]): DeviceAgents[] => {
	const buckets = new Map<string, Agent[]>();
	for (const agent of agents) {
		const key = view.tables.workspace.get(agent.workspace)?.device ?? NO_DEVICE;
		const bucket = buckets.get(key) ?? [];
		bucket.push(agent);
		buckets.set(key, bucket);
	}
	return [...buckets]
		.map(([key, list]) => ({
			key,
			name: key === NO_DEVICE ? t("Unknown device") : deviceName(view, key),
			agents: list,
		}))
		.sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));
};

/** The sections around the project groups. */
export type InboxSections = {
	/** Pinned agents, in their pinned order. */
	readonly pinned: readonly Agent[];
	/** Busy agents, the newest first, while the Working shelf is on. */
	readonly working: readonly Agent[];
	/** Snoozed agents, the soonest to wake first. */
	readonly snoozed: readonly Agent[];
	/** Settled agents, the most recently settled first. */
	readonly settled: readonly Agent[];
};

/** The top-level agents that sit outside the project groups, each list sorted, less a dispatcher's. */
export const inboxSections = (
	view: ViewState,
	now: Timestamp,
	options: SectionOptions = NO_SHELF
): InboxSections => {
	const lists: Record<InboxPlace, Agent[]> = {
		pinned: [],
		active: [],
		working: [],
		snoozed: [],
		settled: [],
	};
	const homes = dispatcherHomes(view);
	for (const agent of all(view, "agent")) {
		if (agent.parent === null && !callerOf(view, agent, now) && !homes.has(agent.workspace)) {
			lists[placeOf(agent, now, options)].push(agent);
		}
	}
	const until = (a: Agent) => inboxOf(a).snoozed_until ?? 0;
	const settledAt = (a: Agent) => inboxOf(a).settled?.at ?? 0;
	return {
		pinned: lists.pinned.sort(byPin),
		working: lists.working.sort(
			(a, b) => b.started_at - a.started_at || a.id.localeCompare(b.id)
		),
		snoozed: lists.snoozed.sort((a, b) => until(a) - until(b) || a.id.localeCompare(b.id)),
		settled: lists.settled.sort(
			(a, b) => settledAt(b) - settledAt(a) || a.id.localeCompare(b.id)
		),
	};
};

/** One entry in the "Needs you" box. */
export type NeedsYouItem = {
	/** The agent asking. */
	readonly agent: Agent;
	/** Its prompt, when the view has it. */
	readonly attention?: AttentionEvent;
	/** The short word on the right, such as "Allow?" or "Question". */
	readonly ask: string;
};

/** Every agent waiting on you, the longest waiting first. */
export const needsYou = (view: ViewState): NeedsYouItem[] =>
	all(view, "agent")
		.filter((a) => a.activity.state === "needs_input")
		.map((agent) => {
			const attention = attentionOf(view, agent);
			const ask =
				attention?.kind === "permission"
					? t("Allow?")
					: attention?.kind === "question"
						? t("Question")
						: t("Input");
			return { agent, attention, ask };
		})
		.sort(
			(a, b) =>
				(a.attention?.created_at ?? a.agent.last_activity_at) -
					(b.attention?.created_at ?? b.agent.last_activity_at) ||
				a.agent.id.localeCompare(b.agent.id)
		);

/** One workspace under a device in the connections view. */
export type WorkspaceNode = {
	/** The workspace. */
	readonly workspace: Workspace;
	/** The container name, or the folder's last part. */
	readonly name: string;
	/** Top-level agents in it, leaving out settled ones. */
	readonly agents: readonly Agent[];
	/** The worst connection among them, if it has any. */
	readonly worst?: ConnectionLook;
};

/** One device in the connections view. */
export type DeviceNode = {
	/** The client's link to it. */
	readonly link: DeviceLink;
	/** Everything the connection helpers need. */
	readonly ctx: LinkContext;
	/** The device's own connection look. */
	readonly look: ConnectionLook;
	/**
	 * Top-level agents on it, leaving out settled ones. MaxHQ stops a
	 * settled agent's process, so its link reads as down without anything
	 * being wrong.
	 */
	readonly agents: readonly Agent[];
	/** Settled top-level agents on it, the most recently settled first. */
	readonly settled: readonly Agent[];
	/** The worst connection among its agents, if it has any. */
	readonly worst?: ConnectionLook;
	/** Its workspaces, by name. Folders on this Mac are left out. */
	readonly workspaces: readonly WorkspaceNode[];
};

/** A group of devices in the connections view. */
export type DeviceGroup = {
	/** "local", a network's ID, or "ssh" for devices on a plain network path. */
	readonly key: string;
	/** The header text. */
	readonly name: string;
	/** The network, for groups that are one. */
	readonly network?: Network;
	/** The devices, by name. */
	readonly devices: readonly DeviceNode[];
};

/** A workspace's short name: its container, or its folder's last part. */
export const workspaceName = (workspace: Workspace): string => {
	if (workspace.location.type === "container") return workspace.location.instance;
	const parts = workspace.location.path.split("/").filter(Boolean);
	return parts[parts.length - 1] ?? workspace.location.path;
};

// The top-level agents a device page counts, leaving out a dispatcher's.
const deviceAgents = (view: ViewState): Agent[] => {
	const homes = dispatcherHomes(view);
	return all(view, "agent").filter((a) => a.parent === null && !homes.has(a.workspace));
};

// One device from the agents `deviceAgents` gave.
const nodeOf = (
	view: ViewState,
	link: DeviceLink,
	agents: readonly Agent[],
	now: Timestamp
): DeviceNode => {
	const ctx = linkContext(view, link);
	const here = agents.filter(
		(a) =>
			link.device !== null && view.tables.workspace.get(a.workspace)?.device === link.device
	);
	const mine = here.filter((a) => inboxOf(a).settled === null);
	const settledAt = (a: Agent) => inboxOf(a).settled?.at ?? 0;
	const settled = here
		.filter((a) => inboxOf(a).settled !== null)
		.sort((a, b) => settledAt(b) - settledAt(a) || a.id.localeCompare(b.id));
	const looks = (list: readonly Agent[]) =>
		worstLook(list.map((a) => agentConnection(a, ctx, now).look));
	const workspaces = all(view, "workspace")
		.filter((w) => w.device === link.device && w.location.type === "container")
		.map((workspace) => {
			const inside = mine.filter((a) => a.workspace === workspace.id);
			return {
				workspace,
				name: workspaceName(workspace),
				agents: inside,
				worst: looks(inside),
			};
		})
		.sort((a, b) => a.name.localeCompare(b.name));
	return {
		link,
		ctx,
		look: deviceConnection(ctx, now).look,
		agents: mine,
		settled,
		worst: looks(mine),
		workspaces,
	};
};

/** One device as the connections view has it, for a page of its own. */
export const deviceNode = (view: ViewState, link: DeviceLink, now: Timestamp): DeviceNode =>
	nodeOf(view, link, deviceAgents(view), now);

/**
 * Devices grouped by how they're reached: this Mac first, then each
 * network, then devices on a plain SSH path. Each device lists its
 * container workspaces with their agent counts (N-5). Settled agents are
 * kept apart, so counts and the worst look cover what's still in view.
 * A dispatcher's agents aren't counted.
 */
export const deviceGroups = (view: ViewState, now: Timestamp): DeviceGroup[] => {
	const agents = deviceAgents(view);
	const node = (link: DeviceLink): DeviceNode => nodeOf(view, link, agents, now);

	const links = all(view, "device_link").sort((a, b) => a.name.localeCompare(b.name));
	const groups: DeviceGroup[] = [];
	const local = links.filter((l) => l.route.type === "local");
	if (local.length > 0)
		groups.push({ key: "local", name: t("This Mac"), devices: local.map(node) });

	const networks = all(view, "network").sort((a, b) => a.name.localeCompare(b.name));
	for (const network of networks) {
		const onIt = links.filter((l) => l.route.type !== "local" && l.network === network.id);
		if (onIt.length > 0) {
			groups.push({ key: network.id, name: network.name, network, devices: onIt.map(node) });
		}
	}
	const known = new Set(networks.map((n) => n.id));
	const plain = links.filter(
		(l) => l.route.type !== "local" && (l.network === null || !known.has(l.network))
	);
	if (plain.length > 0)
		groups.push({ key: "ssh", name: t("Over SSH"), devices: plain.map(node) });
	return groups;
};

// The order vendors list in. The ACP agents come last, since none reports usage.
const VENDOR_ORDER: Record<Vendor, number> = { claude: 0, codex: 1, cursor: 2, acp: 3 };

/** Accounts by vendor, Claude, Codex, Cursor and then ACP agents, and by name within one. */
export const accounts = (view: ViewState): Account[] =>
	all(view, "account").sort(
		(a, b) =>
			VENDOR_ORDER[a.vendor] - VENDOR_ORDER[b.vendor] ||
			a.label.localeCompare(b.label) ||
			a.id.localeCompare(b.id)
	);

/**
 * The models to offer for an account. A new login lists none until its CLI
 * has run under it, so until then it borrows the list of the same vendor's
 * login with the most models, ties going to the lowest id.
 */
export const modelsOf = (view: ViewState, account: Account | undefined): readonly ModelChoice[] => {
	if (!account || account.models.length > 0) return account?.models ?? [];
	let lender: Account | undefined;
	for (const other of view.tables.account.values()) {
		if (other.vendor !== account.vendor || other.models.length === 0) continue;
		if (
			!lender ||
			other.models.length > lender.models.length ||
			(other.models.length === lender.models.length && other.id < lender.id)
		) {
			lender = other;
		}
	}
	return lender?.models ?? [];
};

/** What `vendor`'s logins call `model`, or the model itself when none lists it. */
export const modelName = (view: ViewState, vendor: Vendor, model: string): string => {
	for (const account of view.tables.account.values()) {
		if (account.vendor !== vendor) continue;
		const choice = account.models.find((entry) => entry.id === model);
		if (choice) return choice.label;
	}
	return model;
};

/** An account an agent can move to, with the models to offer there. */
export type Move = { readonly account: Account; readonly models: readonly ModelChoice[] };

/**
 * The accounts a held agent can move to, carrying its conversation over:
 * the other vendors signed in on its device. An ACP agent can also move
 * to another ACP agent, since each is a CLI of its own.
 */
export const movesOf = (view: ViewState, agent: Agent): Move[] => {
	const device = view.tables.workspace.get(agent.workspace)?.device;
	if (agent.control !== "held" || agent.parent !== null || !device) return [];
	return accounts(view)
		.filter(
			(a) =>
				a.device === device &&
				a.id !== agent.account &&
				(a.vendor !== agent.vendor || a.vendor === "acp")
		)
		.map((account) => ({ account, models: modelsOf(view, account) }));
};

/** Top-level agents that run under the account. */
export const agentsOnAccount = (view: ViewState, account: string): Agent[] =>
	all(view, "agent").filter((a) => a.parent === null && a.account === account);

/**
 * How many of the account's agents wait for its limit to reset, counting the
 * agents of `logins` when other logins share its plan.
 */
export const waitingOnAccount = (
	view: ViewState,
	account: Account,
	logins: readonly string[] = [account.id]
): number =>
	atLimit(account)
		? logins
				.flatMap((login) => agentsOnAccount(view, login))
				.filter((a) => a.activity.state === "waiting_for_limit_reset").length
		: 0;

/** The agents whose context windows are fullest, fullest first. */
export const fullestContexts = (view: ViewState, count = 3): Agent[] =>
	all(view, "agent")
		.filter((a) => a.context !== null && a.context.window_tokens > 0)
		.sort(
			(a, b) =>
				b.context!.used_tokens / b.context!.window_tokens -
				a.context!.used_tokens / a.context!.window_tokens
		)
		.slice(0, count);

/** When the newest usage reading was taken, if there is one. */
export const newestUsageCheck = (view: ViewState): Timestamp | undefined => {
	const times = all(view, "account").flatMap((a) => (a.usage ? [a.usage.checked_at] : []));
	return times.length > 0 ? Math.max(...times) : undefined;
};
