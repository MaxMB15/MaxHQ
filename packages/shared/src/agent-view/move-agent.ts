// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Moving an agent to another device. The Mac that runs the MaxHQ app moves
// it: a new agent starts on the other device, under any account signed in
// there, from a summary of the conversation, and the old one stops and
// settles. Files and uncommitted changes stay in the old folder, and the
// tasks that Mac keeps for the agent follow it. The Move dialog picks the
// device, the account, the model and the folder there, with a checkout of
// the same repository picked first, and the banners on both agents point
// at each other. A maxd from before `MOVE_AGENT_SINCE` can't take part.

import { t } from "../i18n";

import {
	type Account,
	type AccountId,
	type Agent,
	type AgentId,
	type DeviceId,
	MOVE_AGENT_SINCE,
	type MoveTo,
	type ProjectId,
	type Vendor,
	type Workspace,
	type WorkspaceId,
} from "../bindings";
import { followLine } from "../dispatch/plan";
import { dispatcherDevice } from "../dispatch/reach";
import {
	accountOptions,
	deviceOptions,
	isAbsolute,
	modelOptions,
	type Option,
	workspaceWhere,
} from "../new-agent/spec";
import { VENDOR_NAME } from "../status/usage";
import { tasksTargeting } from "../tasks/place";
import { all, type ViewState } from "../view/replica";
import {
	deviceName,
	dispatcherHomes,
	linkForDevice,
	modelsOf,
	workspaceName,
} from "../view/select";

import { inTurn } from "./outside";

/** Whether the maxd on `device` takes part in a move. */
export const takesMove = (view: ViewState, device: DeviceId): boolean => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= MOVE_AGENT_SINCE;
};

/** The device an agent runs on, when the view has its folder. */
export const agentDevice = (view: ViewState, agent: Agent): DeviceId | null =>
	view.tables.workspace.get(agent.workspace)?.device ?? null;

/**
 * Whether an agent can move at all: a lead MaxHQ runs that hasn't moved
 * yet, and not one of a dispatcher's.
 */
export const canMove = (view: ViewState, agent: Agent): boolean =>
	agent.control === "held" &&
	agent.parent === null &&
	!agent.moved?.to &&
	!dispatcherHomes(view).has(agent.workspace);

/** Why an agent can't move now, or null when it can. */
export const moveBlocked = (view: ViewState, agent: Agent): string | null => {
	const home = dispatcherDevice(view);
	const from = agentDevice(view, agent);
	if (home === null || from === null) return t("Moving needs the Mac that runs MaxHQ.");
	for (const device of from === home ? [home] : [home, from]) {
		if (!takesMove(view, device)) {
			return t("Moving needs a newer maxd on {{device}}.", {
				device: deviceName(view, device),
			});
		}
	}
	if (all(view, "device").every((device) => device.id === from)) {
		return t("There's no other device to move it to.");
	}
	return null;
};

/** The devices an agent can move to, with the ones that can't take it now turned off. */
export const moveDeviceOptions = (view: ViewState, agent: Agent): Option[] => {
	const from = agentDevice(view, agent);
	return deviceOptions(view)
		.filter((option) => option.value !== from)
		.map((option) =>
			option.disabled || takesMove(view, option.value)
				? option
				: { ...option, detail: t("Needs a newer maxd"), disabled: true }
		);
};

/** A vendor MaxHQ signs in to on a device. */
export type SignInVendor = Exclude<Vendor, "acp">;

const SIGN_INS: readonly SignInVendor[] = ["claude", "codex", "cursor"];

/** The vendors with no account on `device`, each one you can sign in to there. */
export const missingVendors = (view: ViewState, device: DeviceId): SignInVendor[] => {
	const there = new Set(
		all(view, "account")
			.filter((account) => account.device === device)
			.map((account) => account.vendor)
	);
	return SIGN_INS.filter((vendor) => !there.has(vendor));
};

/**
 * The accounts on `device`, the agent's own vendor first, then the vendors
 * not signed in there, turned off.
 */
export const moveAccountOptions = (
	view: ViewState,
	agent: Agent,
	device: DeviceId,
	now: number
): Option[] => {
	const options = accountOptions(view, device, now);
	const own = (option: Option) => view.tables.account.get(option.value)?.vendor === agent.vendor;
	const name = deviceName(view, device);
	return [
		...options.filter(own),
		...options.filter((option) => !own(option)),
		...missingVendors(view, device).map((vendor) => ({
			value: `sign-in:${vendor}`,
			label: VENDOR_NAME[vendor],
			detail: t("Not signed in on {{names}}", { names: name }),
			disabled: true,
		})),
	];
};

/** The pick that moves the agent to a folder you type. */
export const MOVE_FOLDER = "*folder";

const projectOf = (view: ViewState, workspace: Workspace) =>
	workspace.project === null ? undefined : view.tables.project.get(workspace.project);

/**
 * The folders on `device` an agent can move to: the projects' checkouts by
 * name, then the other folders by path, then one you type.
 */
export const movePlaceOptions = (view: ViewState, device: DeviceId): Option[] => {
	const homes = dispatcherHomes(view);
	const here = all(view, "workspace").filter((w) => w.device === device && !homes.has(w.id));
	const checkouts = here
		.flatMap((w) => {
			const project = projectOf(view, w);
			return project ? [{ w, name: project.name }] : [];
		})
		.sort((a, b) => a.name.localeCompare(b.name) || a.w.id.localeCompare(b.w.id))
		.map(({ w, name }) => ({
			value: w.id,
			label: name,
			detail: workspaceWhere(w),
			group: t("Projects"),
		}));
	const folders = here
		.filter((w) => projectOf(view, w) === undefined)
		.sort((a, b) => workspaceWhere(a).localeCompare(workspaceWhere(b)))
		.map((w) => ({
			value: w.id,
			label: workspaceName(w),
			detail: workspaceWhere(w),
			group: t("Folders"),
		}));
	return [
		...checkouts,
		...folders,
		{
			value: MOVE_FOLDER,
			label: t("Another folder…"),
			detail: t("Type its path on {{device}}", { device: deviceName(view, device) }),
			group: t("Folders"),
		},
	];
};

// A clone URL in one form, so the https, SSH and bare forms of one
// repository match.
const repoKey = (repo: string): string =>
	repo
		.trim()
		.toLowerCase()
		.replace(/^[a-z+]+:\/\//, "")
		.replace(/^[^@/]+@/, "")
		.replace(/^([^/:]+):/, "$1/")
		.replace(/\.git$/, "")
		.replace(/\/+$/, "");

/**
 * The one of `places` that checks out the same repository as `project`:
 * that project's checkout first, then one with the same remote.
 */
export const sameRepoIn = (
	view: ViewState,
	project: ProjectId,
	places: readonly Workspace[]
): Workspace | undefined => {
	const repo = view.tables.project.get(project)?.repo;
	const key = repo ? repoKey(repo) : null;
	const sameRemote = (w: Workspace) => {
		const theirs = projectOf(view, w)?.repo;
		return key !== null && theirs != null && repoKey(theirs) === key;
	};
	return places.find((w) => w.project === project) ?? places.find(sameRemote);
};

/**
 * The folder on `device` that checks out the same repository as the
 * agent's folder: the same project's checkout first, then one with the
 * same remote. Null when there's none.
 */
export const sameRepoPlace = (
	view: ViewState,
	agent: Agent,
	device: DeviceId
): WorkspaceId | null => {
	const workspace = view.tables.workspace.get(agent.workspace);
	const project = workspace ? projectOf(view, workspace) : undefined;
	if (!project) return null;
	const places = movePlaceOptions(view, device).flatMap((option) => {
		const w = view.tables.workspace.get(option.value);
		return w ? [w] : [];
	});
	return sameRepoIn(view, project.id, places)?.id ?? null;
};

/** The tasks that follow a moved agent: the ones the Mac that moves it keeps. */
export const followingTasks = (view: ViewState, agent: Agent): number => {
	const home = dispatcherDevice(view);
	return tasksTargeting(view, agent.id).filter((task) => task.home === home).length;
};

/** What you picked in the Move dialog. Unset fields take their default. */
export type MoveChoices = {
	readonly device: DeviceId | null;
	readonly account: AccountId | null;
	/** Unset keeps the agent's model when it stays on its vendor, else the default. */
	readonly model: string | null;
	/** A workspace there, or `MOVE_FOLDER` for a folder you type. */
	readonly place: string | null;
	/** The folder's absolute path, for `MOVE_FOLDER`. */
	readonly path: string;
};

/** The Move dialog before you pick anything. */
export const NO_MOVE_CHOICES: MoveChoices = {
	device: null,
	account: null,
	model: null,
	place: null,
	path: "",
};

/** The Move dialog as it stands. */
export type MoveForm = {
	readonly devices: readonly Option[];
	readonly device: DeviceId | null;
	readonly accounts: readonly Option[];
	readonly account: Account | undefined;
	/** The vendors not signed in on the device, each a way to sign in there. */
	readonly missing: readonly SignInVendor[];
	readonly models: readonly Option[];
	/** The model shown, empty for the account's default when it lists none. */
	readonly model: string;
	readonly places: readonly Option[];
	/** The workspace or `MOVE_FOLDER` picked, or null before there's one. */
	readonly place: string | null;
	/** What the move does, a sentence each. */
	readonly lines: readonly string[];
	/** Whether a turn runs that the move stops first. */
	readonly stop: boolean;
	/** The confirm button's words: Stop and move while a turn runs. */
	readonly action: string;
	/** Why it can't move yet, or null when it can. */
	readonly problem: string | null;
	/** Where it goes, once nothing is in the way. */
	readonly to: MoveTo | null;
};

const firstUsable = (options: readonly Option[], picked: string | null): string | null =>
	options.find((o) => o.value === picked && !o.disabled)?.value ??
	options.find((o) => !o.disabled)?.value ??
	null;

/** What the move does, a sentence each, for an agent going to `device` under `account`. */
export const moveLines = (
	view: ViewState,
	agent: Agent,
	device: DeviceId | null,
	account: Account | undefined
): string[] => {
	const from = agentDevice(view, agent);
	const fromName = from === null ? t("Unknown device") : deviceName(view, from);
	const workspace = view.tables.workspace.get(agent.workspace);
	const follow = followingTasks(view, agent);
	return [
		t("Only a summary of the conversation carries over."),
		workspace
			? t("Files and uncommitted changes in {{folder}} stay on {{device}}.", {
					folder: workspaceName(workspace),
					device: fromName,
				})
			: t("Files and uncommitted changes stay on {{device}}.", { device: fromName }),
		t("This agent stops and settles on {{device}}.", { device: fromName }),
		...(inTurn(agent) ? [t("Its current turn stops.")] : []),
		...(account && account.vendor !== agent.vendor
			? [
					t("It goes on with {{vendor}} on {{device}}.", {
						vendor: VENDOR_NAME[account.vendor],
						device: device === null ? "" : deviceName(view, device),
					}),
				]
			: []),
		...(follow > 0 ? [`${followLine(follow)}.`] : []),
	];
};

/** The Move dialog for an agent, from what you picked. */
export const moveForm = (
	view: ViewState,
	agent: Agent,
	choices: MoveChoices,
	now: number
): MoveForm => {
	const devices = moveDeviceOptions(view, agent);
	const device = firstUsable(devices, choices.device);
	const accounts = device === null ? [] : moveAccountOptions(view, agent, device, now);
	const accountId = firstUsable(accounts, choices.account);
	const account = accountId === null ? undefined : view.tables.account.get(accountId);
	const listed = modelsOf(view, account);
	const wanted = choices.model ?? (account?.vendor === agent.vendor ? agent.model : null);
	const model = listed.some((m) => m.id === wanted) ? wanted : null;
	const places = device === null ? [] : movePlaceOptions(view, device);
	const place = places.some((o) => o.value === choices.place)
		? choices.place
		: device === null
			? null
			: sameRepoPlace(view, agent, device);
	const path = choices.path.trim();
	const stop = inTurn(agent);
	const name = device === null ? "" : deviceName(view, device);
	const problem =
		moveBlocked(view, agent) ??
		(device === null
			? t("No other device can take it now.")
			: !account
				? t("Sign in on {{device}} first.", { device: name })
				: place === null
					? t("Pick a folder on {{device}}.", { device: name })
					: place === MOVE_FOLDER && !isAbsolute(path)
						? t("Type the folder's full path, starting with /")
						: null);
	const to: MoveTo | null =
		problem === null && device !== null && account && place !== null
			? {
					device,
					account: account.id,
					model,
					place:
						place === MOVE_FOLDER
							? { type: "folder", path }
							: { type: "workspace", workspace: place },
				}
			: null;
	return {
		devices,
		device,
		accounts,
		account,
		missing: device === null ? [] : missingVendors(view, device),
		models: modelOptions(view, account),
		model: model ?? listed.find((m) => m.default)?.id ?? listed[0]?.id ?? "",
		places,
		place,
		lines: moveLines(view, agent, device, account),
		stop,
		action: stop ? t("Stop and move") : t("Move"),
		problem,
		to,
	};
};

/** What a moved agent's banner says, and the agent on the other side. */
export type MovedBanner = {
	/** Moved to another device, or moved here from one. */
	readonly way: "to" | "from";
	readonly title: string;
	readonly detail: string;
	/** The agent on the other device, when the view still has it. */
	readonly other: AgentId | null;
};

/** The banner of an agent that moved away or moved here, or null for one that didn't. */
export const movedBanner = (view: ViewState, agent: Agent): MovedBanner | null => {
	const { to, from } = agent.moved ?? {};
	const at = to ?? from;
	if (!at) return null;
	const device = deviceName(view, at.device);
	const other = view.tables.agent.has(at.agent) ? at.agent : null;
	return to
		? {
				way: "to",
				title: t("Moved to {{device}}", { device }),
				detail: t(
					"This agent stopped. It goes on as a new agent on {{device}}, from a summary of this conversation.",
					{ device }
				),
				other,
			}
		: {
				way: "from",
				title: t("Moved from {{device}}", { device }),
				detail: t(
					"It started from a summary of the conversation there. Files and uncommitted changes stayed on {{device}}.",
					{ device }
				),
				other,
			};
};
