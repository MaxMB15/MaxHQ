// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the new-agent form offers and what it sends. The form keeps what you
// picked, and `resolve` turns that into a consistent set against the view:
// a device from the project's checkout or the folder's device, a login on
// that device, a model that login has and a mode its CLI can run. Anything
// you haven't picked, or that no longer fits, falls back to a default, and
// the defaults lean toward a place where an agent can start now. When more
// than one device could take the agent and you haven't picked, the one with
// the most free CPU and memory for its placement wins.

import { t } from "../i18n";

import type {
	Account,
	AccountId,
	AcpFeatures,
	AgentPlace,
	AgentSpec,
	DeviceId,
	PermissionMode,
	ProjectId,
	Vendor,
	Workspace,
	WorkspaceId,
} from "../bindings";
import type { ActionResult, DeviceLoad } from "../data/source";
import { NOT_OFFERED } from "../agent-view/controls";
import { chooseDevice, PLACEMENT_WEIGHT } from "../fleet/balance";
import { branchProblem, folderClash } from "./branch";
import { agentOfAccount } from "../settings/acp-agents";
import {
	MODE_HINT,
	MODE_LABEL,
	MODES,
	modeShown,
	startingMode,
	vendorHasMode,
} from "../settings/permission";
import {
	localAccountOption,
	localModelOptions,
	resolveLocal,
	shownLocalModel,
} from "../local-models/select";
import { isLocalAccount } from "../status/agent-kind";
import { formatClock, plural } from "../status/format";
import type { Tone } from "../tasks/format";
import { accountName, accountWindows, blockingWindow, type WindowView } from "../status/usage";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice, modelsOf } from "../view/select";

/** What you picked in the form. Unset fields take their default. */
export type Choices = {
	/** A project's checkout, or any folder on a device. */
	readonly place: "project" | "folder";
	readonly project: ProjectId | null;
	readonly workspace: WorkspaceId | null;
	/** The device a folder is on. */
	readonly device: DeviceId | null;
	/** The folder's absolute path on its device. */
	readonly path: string;
	readonly account: AccountId | null;
	/** Unset runs the login's default model. */
	readonly model: string | null;
	/** Unset starts in the device's default mode. */
	readonly mode: PermissionMode | null;
	readonly title: string;
	readonly first: string;
	/** The branch for an agent in a container. Empty lets `maxd` name one. */
	readonly branch: string;
};

/** One entry in a picker. */
export type Option = {
	readonly value: string;
	readonly label: string;
	/** A second line under the label. */
	readonly detail?: string;
	/** Draws the second line as a warning. */
	readonly warn?: boolean;
	/** A short tag on the right, such as "Not in Codex". */
	readonly tag?: string;
	/** The tag's color, such as green for a local model that fits. Plain when not set. */
	readonly tone?: Tone;
	readonly disabled?: boolean;
	/** The heading of the group the entry sits under. */
	readonly group?: string;
	/** A login's own usage windows, drawn as bars on the right. */
	readonly usage?: readonly WindowView[];
};

/** The form's choices made consistent with the view. */
export type Resolved = {
	readonly place: "project" | "folder";
	readonly project: ProjectId | null;
	readonly workspace: Workspace | undefined;
	readonly device: DeviceId | null;
	readonly path: string;
	readonly account: Account | undefined;
	/** The model to send, or null for the login's default. */
	readonly model: string | null;
	/** The model the picker shows, or null when the login hasn't listed any. */
	readonly shownModel: string | null;
	readonly mode: PermissionMode;
	readonly title: string | null;
	readonly first: string | null;
	/** The branch to send, or null for a new one `maxd` names. */
	readonly branch: string | null;
	/** Whether the device is the balancer's pick, not yours or the project's. */
	readonly balanced: boolean;
};

/** The form you get with nothing picked, for a project or for the folder of nothing. */
export const emptyChoices = (project: ProjectId | null = null): Choices => ({
	place: "project",
	project,
	workspace: null,
	device: null,
	path: "",
	account: null,
	model: null,
	mode: null,
	title: "",
	first: "",
	branch: "",
});

const connected = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.state.state === "connected";

// Whether an agent could start on the device right now.
const ready = (view: ViewState, device: DeviceId): boolean =>
	connected(view, device) && all(view, "account").some((a) => a.device === device);

const workspacesOf = (view: ViewState, project: ProjectId): Workspace[] =>
	all(view, "workspace")
		.filter((w) => w.project === project)
		.sort((a, b) => a.id.localeCompare(b.id));

const agentsOn = (view: ViewState, device: DeviceId): number =>
	all(view, "agent").filter(
		(a) => a.parent === null && view.tables.workspace.get(a.workspace)?.device === device
	).length;

/** Whether a workspace is a container, where each agent gets its own branch. */
export const inContainer = (workspace: Workspace | undefined): boolean =>
	workspace?.location.type === "container";

/** Where a workspace lives, in a few words. */
export const workspaceWhere = (workspace: Workspace): string =>
	workspace.location.type === "container"
		? t("{{value1}} container", { value1: workspace.location.instance })
		: workspace.location.path;

/** Projects that have a checkout somewhere, by name. */
export const projectOptions = (view: ViewState): Option[] =>
	all(view, "project")
		.map((project) => ({ project, places: workspacesOf(view, project.id) }))
		.filter(({ places }) => places.length > 0)
		.sort((a, b) => a.project.name.localeCompare(b.project.name))
		.map(({ project, places }) => ({
			value: project.id,
			label: project.name,
			detail: [...new Set(places.map((w) => deviceName(view, w.device)))].join(", "),
		}));

/** The project's checkouts, each on its device. */
export const workspaceOptions = (view: ViewState, project: ProjectId): Option[] =>
	workspacesOf(view, project).map((w) => {
		const up = connected(view, w.device);
		return {
			value: w.id,
			label: deviceName(view, w.device),
			detail: up ? workspaceWhere(w) : t("Not connected"),
			disabled: !up,
		};
	});

/** Every device the client knows, with the ones that aren't connected turned off. */
export const deviceOptions = (view: ViewState): Option[] =>
	all(view, "device")
		.map((device) => ({ device, up: connected(view, device.id) }))
		.sort((a, b) => Number(b.up) - Number(a.up) || a.device.id.localeCompare(b.device.id))
		.map(({ device, up }) => {
			const name = deviceName(view, device.id);
			const own = device.name !== name ? `${device.name}, ` : "";
			return {
				value: device.id,
				label: name,
				detail: up
					? `${own}${plural(agentsOn(view, device.id), "agent")}`
					: t("Not connected"),
				disabled: !up,
			};
		});

const CLI_NAME: Record<Vendor, string> = {
	claude: "Claude Code",
	get codex() {
		return t("Codex CLI");
	},
	get cursor() {
		return t("Cursor CLI");
	},
	get acp() {
		return t("ACP agent");
	},
};

// What an account's detail names after the CLI: the email or label, or the
// program a local ACP agent runs.
const whoOf = (view: ViewState, account: Account): string =>
	account.vendor === "acp"
		? (agentOfAccount(view, account)?.program ?? account.label)
		: (account.email ?? account.label);

/**
 * The logins on a device, which pick the vendor too. The device's local
 * models come last as one Local entry, standing for `selected` when that's
 * one of its local accounts.
 */
export const accountOptions = (
	view: ViewState,
	device: DeviceId,
	now: number,
	selected: AccountId | null = null
): Option[] => {
	const local = localAccountOption(view, device, selected);
	return all(view, "account")
		.filter((a) => a.device === device && !isLocalAccount(a))
		.sort((a, b) => a.vendor.localeCompare(b.vendor) || a.id.localeCompare(b.id))
		.map((account): Option => {
			const blocked = blockingWindow(account);
			const until =
				blocked?.resetsAt != null && blocked.resetsAt > now
					? t(" until {{value1}}", { value1: formatClock(blocked.resetsAt) })
					: "";
			return {
				value: account.id,
				label: accountName(account),
				detail: blocked
					? t("{{CLI_NAME_account_vendor}}, at its limit{{until}}", {
							CLI_NAME_account_vendor: CLI_NAME[account.vendor],
							until: until,
						})
					: `${CLI_NAME[account.vendor]}, ${whoOf(view, account)}`,
				warn: blocked !== undefined,
				usage: accountWindows(account),
			};
		})
		.concat(local ? [local] : []);
};

/**
 * The models a login offers, with its default marked. A local login offers
 * the models of every local login on its device, grouped by server, so
 * picking one can move to another server's login; `modelChoice` reads the
 * value back.
 */
export const modelOptions = (view: ViewState, account: Account | undefined): Option[] => {
	if (account && isLocalAccount(account)) return localModelOptions(view, account.device);
	const models = modelsOf(view, account);
	if (models.length === 0) {
		return account?.vendor === "acp"
			? [
					{
						value: "",
						label: t("Agent default"),
						detail: t("The agent didn't list its models"),
					},
				]
			: [
					{
						value: "",
						label: t("CLI default"),
						detail: t("The CLI hasn't listed its models yet"),
					},
				];
	}
	return models.map((m) => ({
		value: m.id,
		label: m.label,
		detail: m.default ? t("The default") : undefined,
	}));
};

/**
 * The five modes, with Plan turned off for Codex. A local ACP agent gets
 * only the ones it can run, as `acp` says.
 */
export const modeOptions = (vendor: Vendor | undefined, acp?: AcpFeatures | null): Option[] =>
	MODES.filter((mode) => vendor === undefined || modeShown(vendor, mode, acp)).map((mode) => {
		const can = vendor === undefined || vendorHasMode(vendor, mode, acp);
		return {
			value: mode,
			label: MODE_LABEL[mode],
			detail: MODE_HINT[mode],
			warn: mode === "full_access",
			tag: mode === "plan" && (vendor === undefined || !can) ? NOT_OFFERED : undefined,
			disabled: !can,
		};
	});

const firstUsable = (options: readonly Option[]): string | undefined =>
	options.find((o) => !o.disabled)?.value ?? options[0]?.value;

// The device a folder goes on: the one you picked while it's connected,
// else this Mac, else any connected device.
const folderDevice = (view: ViewState, wanted: DeviceId | null): DeviceId | null => {
	if (wanted && view.tables.device.has(wanted)) return wanted;
	const local = all(view, "device_link").find(
		(l) => l.route.type === "local" && l.device && connected(view, l.device)
	);
	if (local?.device) return local.device;
	return firstUsable(deviceOptions(view)) ?? null;
};

// The login to use on a device: the one you picked, else the project's
// default, else the first for the project's default vendor, else the first.
// A local login is only ever picked by name, never as a vendor's or the
// device's first.
const pickAccount = (
	view: ViewState,
	device: DeviceId | null,
	wanted: AccountId | null,
	project: ProjectId | null
): Account | undefined => {
	if (device === null) return undefined;
	const here = all(view, "account")
		.filter((a) => a.device === device)
		.sort(
			(a, b) =>
				Number(isLocalAccount(a)) - Number(isLocalAccount(b)) ||
				a.vendor.localeCompare(b.vendor) ||
				a.id.localeCompare(b.id)
		);
	const settings = project ? view.tables.project.get(project) : undefined;
	return (
		here.find((a) => a.id === wanted) ??
		here.find((a) => a.id === settings?.default_account) ??
		here.find((a) => a.vendor === settings?.default_vendor && !isLocalAccount(a)) ??
		here[0]
	);
};

// The vendor the agent would run: the login you picked, else the project's.
const wantedVendor = (view: ViewState, choices: Choices): Vendor | null => {
	const picked = choices.account ? view.tables.account.get(choices.account) : undefined;
	if (picked) return picked.vendor;
	const project = choices.project ? view.tables.project.get(choices.project) : undefined;
	return choices.place === "project" ? (project?.default_vendor ?? null) : null;
};

/**
 * The devices the balancer weighs for these choices: connected ones with a
 * login for the vendor that hold the project's checkout, or any such device
 * for a folder. Empty once you've picked the place, the project names its
 * own default, or only one device fits, since then there's nothing to weigh.
 */
export const balanceCandidates = (view: ViewState, choices: Choices): DeviceId[] => {
	const vendor = wantedVendor(view, choices);
	const fits = (device: DeviceId) =>
		connected(view, device) &&
		all(view, "account").some(
			(a) => a.device === device && (vendor === null || a.vendor === vendor)
		);
	let devices: DeviceId[];
	if (choices.place === "folder") {
		if (choices.device) return [];
		devices = all(view, "device").map((d) => d.id);
	} else {
		const project = resolveProject(view, choices);
		if (!project || choices.workspace) return [];
		const defaultWs = view.tables.project.get(project)?.default_workspace;
		const own = defaultWs ? view.tables.workspace.get(defaultWs) : undefined;
		if (own && ready(view, own.device)) return [];
		devices = workspacesOf(view, project).map((w) => w.device);
	}
	const fitting = [...new Set(devices)].filter(fits).sort();
	return fitting.length > 1 ? fitting : [];
};

/** The candidate with the most free CPU and memory for its placement, from fresh samples. */
export const pickBalanced = (
	view: ViewState,
	candidates: readonly DeviceId[],
	loads: readonly DeviceLoad[],
	now: number
): DeviceId | null =>
	chooseDevice(
		candidates.map((device) => {
			const sample = loads.find((l) => l.device === device);
			const placement = view.tables.device_settings.get(device)?.placement ?? "normal";
			return {
				device,
				load: sample?.load ?? null,
				receivedAt: sample?.received_at,
				weight: PLACEMENT_WEIGHT[placement],
			};
		}),
		now
	);

/** What the form says about a device the balancer picked. */
export const balanceNote = (view: ViewState, device: DeviceId): string =>
	t("{{deviceName_view__device}}: most free CPU and memory", {
		deviceName_view__device: deviceName(view, device),
	});

const resolveProject = (view: ViewState, choices: Choices): ProjectId | null => {
	const projects = choices.place === "folder" ? [] : projectOptions(view);
	return (
		projects.find((o) => o.value === choices.project)?.value ??
		projects.find((o) => workspacesOf(view, o.value).some((w) => ready(view, w.device)))
			?.value ??
		projects[0]?.value ??
		null
	);
};

/**
 * The choices made consistent with the view, ready to show and to send.
 * `balanced` is the balancer's pick, used only where you haven't picked.
 */
export const resolve = (
	view: ViewState,
	choices: Choices,
	balanced: DeviceId | null = null
): Resolved => {
	const folder = choices.place === "folder";
	const project = resolveProject(view, choices);
	// Weighed only when the choices still leave it open.
	const weighed =
		balanced !== null && balanceCandidates(view, choices).includes(balanced) ? balanced : null;

	let workspace: Workspace | undefined;
	if (project) {
		const places = workspaceOptions(view, project);
		const defaultWs = view.tables.project.get(project)?.default_workspace;
		const id =
			places.find((o) => o.value === choices.workspace)?.value ??
			places.find((o) => o.value === defaultWs && !o.disabled)?.value ??
			(weighed
				? workspacesOf(view, project).find((w) => w.device === weighed)?.id
				: undefined) ??
			workspacesOf(view, project).find((w) => ready(view, w.device))?.id ??
			firstUsable(places);
		workspace = id ? view.tables.workspace.get(id) : undefined;
	}

	const device = folder
		? folderDevice(view, choices.device ?? weighed)
		: (workspace?.device ?? null);
	const picked = pickAccount(view, device, choices.account, project);
	// A local login always sends its model, since it has no CLI default.
	const local =
		picked && device !== null && isLocalAccount(picked)
			? resolveLocal(view, device, picked, choices.model)
			: null;
	const account = local?.account ?? picked;
	const models = modelsOf(view, account);
	const model = local
		? local.model
		: models.some((m) => m.id === choices.model)
			? choices.model
			: null;
	const shownModel = local
		? shownLocalModel(local.account, local.model)
		: (model ?? models.find((m) => m.default)?.id ?? models[0]?.id ?? null);
	const vendor = account?.vendor;
	const mode =
		choices.mode && (!vendor || vendorHasMode(vendor, choices.mode, account?.acp))
			? choices.mode
			: device
				? startingMode(view, device, vendor ?? "claude", account?.acp)
				: "ask";

	return {
		place: choices.place,
		project,
		workspace,
		device,
		path: choices.path.trim(),
		account,
		model,
		shownModel,
		mode,
		title: choices.title.trim() || null,
		first: choices.first.trim() || null,
		branch: inContainer(workspace) ? choices.branch.trim() || null : null,
		balanced: weighed !== null && device === weighed,
	};
};

/**
 * The choices after a pick. A branch applies only in a container, so it's
 * dropped once the pick moves the place out of one.
 */
export const pickChoices = (
	view: ViewState,
	choices: Choices,
	patch: Partial<Choices>,
	balanced: DeviceId | null = null
): Choices => {
	const next = { ...choices, ...patch };
	if (next.branch === "" || inContainer(resolve(view, next, balanced).workspace)) return next;
	return { ...next, branch: "" };
};

/** Whether a path is absolute, on Unix or on Windows. */
export const isAbsolute = (path: string): boolean =>
	path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path);

/** What starting would send, or why it can't start yet. */
export type Plan =
	| { readonly ok: true; readonly spec: AgentSpec }
	| { readonly ok: false; readonly reason: string };

/** The spec for the agent the form describes. */
export const planStart = (
	view: ViewState,
	choices: Choices,
	balanced: DeviceId | null = null
): Plan => {
	const r = resolve(view, choices, balanced);
	if (r.place === "project") {
		if (!r.project) return { ok: false, reason: t("Pick a project, or use a local folder") };
		if (!r.workspace) return { ok: false, reason: t("This project has no checkout yet") };
	}
	if (!r.device) return { ok: false, reason: t("Connect a device first") };
	const name = deviceName(view, r.device);
	if (!connected(view, r.device))
		return { ok: false, reason: t("{{name}} isn't connected", { name: name }) };
	if (r.place === "folder" && !isAbsolute(r.path)) {
		return { ok: false, reason: t("Type the folder's full path, starting with /") };
	}
	if (!r.account)
		return {
			ok: false,
			reason: t("{{name}} has no Claude, Codex or Cursor login", { name: name }),
		};
	const badBranch =
		r.branch === null
			? null
			: (branchProblem(r.branch) ??
				(r.workspace ? folderClash(view, r.workspace.id, r.branch) : null));
	if (badBranch) return { ok: false, reason: badBranch };
	const place: AgentPlace = r.workspace
		? { type: "workspace", workspace: r.workspace.id }
		: { type: "folder", path: r.path };
	return {
		ok: true,
		spec: {
			vendor: r.account.vendor,
			account: r.account.id,
			place,
			model: r.model,
			permission: r.mode,
			title: r.title,
			first: r.first ? { text: r.first, attachments: [] } : null,
			branch: r.branch,
		},
	};
};

/** The words for a reply that didn't start an agent. */
export const startFailure = (result: ActionResult): string => {
	if (result.status === "error") {
		const { error } = result;
		switch (error.kind) {
			case "not_connected":
				return t("{{error_device}} isn't connected", { error_device: error.device });
			case "not_found":
				return t("Couldn't find {{error_what}}", { error_what: error.what });
			case "failed":
				return error.message;
		}
	}
	return result.data.status === "refused" ? result.data.reason : t("It didn't start");
};
