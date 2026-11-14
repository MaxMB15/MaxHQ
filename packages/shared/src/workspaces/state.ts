// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Container workspaces in words: what each is doing, the dot N-2 gives it,
// what you can do to it from there, and the spec a new one is made from.
// Green is running, orange is a change in flight, purple is a container the
// device answers for but that doesn't run, such as one stopped or with its
// storage locked, and red is one that failed.

import { t } from "../i18n";

import type {
	Capabilities,
	DeviceId,
	EgressCheck,
	GuestState,
	ManagedWorkspace,
	ProjectId,
	Timestamp,
	Workspace,
	WorkspaceSpec,
	WorkspaceState,
} from "../bindings";
import type { CloneProgress, CloneStage, CloneState } from "../data/source";
import type { Option } from "../new-agent/spec";
import { type ConnectionLook, deviceConnection } from "../status/connection";
import { formatWhen, listInWords, sentence } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkContext, linkForDevice } from "../view/select";

/** Whether a device's `maxd` answers, so it can take a change. */
export const isLive = (view: ViewState, device: DeviceId, now: Timestamp): boolean => {
	const link = linkForDevice(view, device);
	return !!link && deviceConnection(linkContext(view, link), now).look === "live";
};

/**
 * Whether a device's `maxd` makes container workspaces, as its hello said.
 * A link that hasn't heard from its device yet knows nothing, so Incus on
 * the device decides.
 */
export const makesWorkspaces = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.workspaces !== false;

/**
 * Whether a device's `maxd` lists, fetches and places files for a copy, and
 * clones into folders. A `maxd` from before copies leaves it out.
 */
export const canCopy = (view: ViewState, device: DeviceId): boolean =>
	(
		linkForDevice(view, device)?.capabilities as
			(Capabilities & { copy?: boolean }) | null | undefined
	)?.copy === true;

/** Where a clone into a folder workspace is, or null once it's done. */
export const cloneOf = (workspace: Workspace): CloneState | null =>
	(workspace as Workspace & { clone?: CloneProgress | null }).clone?.state ?? null;

const STAGE_LABEL: Record<CloneStage, string> = {
	get connecting() {
		return t("reaching the remote");
	},
	get counting() {
		return t("counting objects");
	},
	get receiving() {
		return t("receiving objects");
	},
	get resolving() {
		return t("resolving deltas");
	},
	get checkout() {
		return t("checking out files");
	},
};

/** A clone in words, such as "Cloning: receiving objects, 42%", or null once it's done. */
export const cloneLabel = (clone: CloneState | null): string | null => {
	if (!clone) return null;
	switch (clone.state) {
		case "cloning": {
			const at = clone.percent === null ? "" : `, ${clone.percent}%`;
			return clone.stalled
				? t(
						"Clone stalled{{clone_percent_____null__________at___clone_pe}}. git has said nothing for a while",
						{
							clone_percent_____null__________at___clone_pe:
								clone.percent === null
									? ""
									: t(" at {{value1}}%", { value1: clone.percent }),
						}
					)
				: t("Cloning: {{STAGE_LABEL_clone_stage}}{{at}}", {
						STAGE_LABEL_clone_stage: STAGE_LABEL[clone.stage],
						at: at,
					});
		}
		case "failed":
			return t("Clone failed: {{sentence_clone_reason}}", {
				sentence_clone_reason: sentence(clone.reason),
			});
		case "cancelled":
			return t("Clone cancelled");
	}
};

type Made = {
	readonly managed: ManagedWorkspace;
	readonly location: Extract<Workspace["location"], { type: "container" }>;
};

/** A workspace MaxHQ made, with its container. */
export type MadeWorkspace = Workspace & Made;

/** Whether MaxHQ made and runs the workspace. */
export const isMade = <W extends Workspace>(workspace: W): workspace is W & Made =>
	!!workspace.managed && workspace.location.type === "container";

/** The workspaces MaxHQ made, by container name. */
export const madeWorkspaces = (
	view: ViewState,
	where: { device?: DeviceId; project?: ProjectId }
): MadeWorkspace[] =>
	all(view, "workspace")
		.filter(isMade)
		.filter(
			(w) =>
				(where.device === undefined || w.device === where.device) &&
				(where.project === undefined || w.project === where.project)
		)
		.sort((a, b) => a.location.instance.localeCompare(b.location.instance));

/** Each state's name. */
export const STATE_LABEL: Record<WorkspaceState["state"], string> = {
	get creating() {
		return t("Creating");
	},
	get starting() {
		return t("Starting");
	},
	get running() {
		return t("Running");
	},
	get stopping() {
		return t("Stopping");
	},
	get stopped() {
		return t("Stopped");
	},
	get deleting() {
		return t("Deleting");
	},
	get storage_locked() {
		return t("Storage locked");
	},
	get failed() {
		return t("Failed");
	},
};

/** The dot a state gets. */
export const stateLook = (state: WorkspaceState): ConnectionLook => {
	switch (state.state) {
		case "running":
			return "live";
		case "creating":
		case "starting":
		case "stopping":
		case "deleting":
			return "connecting";
		case "stopped":
		case "storage_locked":
			return "agent_down";
		case "failed":
			return "unreachable";
	}
};

/** The text classes for a state's words, matching its dot. */
export const STATE_TONE: Record<ConnectionLook, string> = {
	live: "text-status-ready-text",
	asleep: "text-muted-foreground",
	connecting: "text-status-connecting-text",
	agent_down: "text-status-linked",
	unreachable: "text-status-error-text",
	unknown: "text-muted-foreground",
};

/** What the state line says, such as "Creating, step 2 of 5: Checking what it can reach". */
export const stateText = (state: WorkspaceState): string => {
	switch (state.state) {
		case "creating":
			return t(
				"Creating, step {{Math_min_state_done___1__state_of}} of {{state_of}}: {{state_step}}",
				{
					Math_min_state_done___1__state_of: Math.min(state.done + 1, state.of),
					state_of: state.of,
					state_step: state.step,
				}
			);
		case "storage_locked":
			return t("Storage locked. Unlock the device's storage, then start it");
		case "failed":
			return t("Failed: {{sentence_state_reason}}", {
				sentence_state_reason: sentence(state.reason),
			});
		default:
			return STATE_LABEL[state.state];
	}
};

/** Whether the guest inside answers, in words. */
export const guestText = (guest: GuestState, now: Timestamp): string => {
	switch (guest.state) {
		case "connected":
			return t("maxd guest {{guest_version}}", { guest_version: guest.version });
		case "installing":
			return t("Installing maxd in the container");
		case "down":
			return t("Guest down since {{formatWhen_guest_since__now}}: {{guest_reason}}", {
				formatWhen_guest_since__now: formatWhen(guest.since, now),
				guest_reason: guest.reason,
			});
	}
};

/** The egress probe's result, with what answered when it failed. */
export const egressText = (egress: EgressCheck, now: Timestamp): string => {
	switch (egress.state) {
		case "not_run":
			return t("Egress check not run yet");
		case "passed":
			return t(
				"Can't reach your LAN or tailnet. The public internet stays open. Checked at {{formatWhen_egress_at__now}}",
				{ formatWhen_egress_at__now: formatWhen(egress.at, now) }
			);
		case "failed":
			return egress.reached.length > 0
				? t(
						"Egress check failed at {{formatWhen_egress_at__now}}. {{listInWords_egress_reached}} answered, so agents won't start until a check passes",
						{
							formatWhen_egress_at__now: formatWhen(egress.at, now),
							listInWords_egress_reached: listInWords(egress.reached),
						}
					)
				: t(
						"Egress check failed at {{formatWhen_egress_at__now}}, so agents won't start until a check passes",
						{ formatWhen_egress_at__now: formatWhen(egress.at, now) }
					);
	}
};

/** Something you can do to a workspace. */
export type WorkspaceAction = "start" | "stop" | "delete";

/** What you can do to a workspace in its state. Nothing while a change runs. */
export const workspaceActions = (state: WorkspaceState): WorkspaceAction[] => {
	switch (state.state) {
		case "running":
			return ["stop", "delete"];
		case "stopped":
		case "failed":
			return ["start", "delete"];
		case "storage_locked":
			return ["delete"];
		default:
			return [];
	}
};

/** What you pick for a new workspace. */
export type WorkspaceChoices = {
	readonly project: ProjectId | null;
	readonly device: DeviceId | null;
	/** The container's name after `mhq-`, or empty for the project's name. */
	readonly name: string;
	readonly startAtBoot: boolean;
};

/** The name a project gives its container when you don't pick one. */
export const defaultName = (projectName: string): string =>
	projectName
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-+|-+$/g, "");

// Incus names run to 63 characters, and `mhq-` takes four.
const NAME = /^[a-z0-9]([a-z0-9-]{0,57}[a-z0-9])?$/;

/** The spec to send, or why there isn't one yet. */
export type WorkspacePlan =
	| { readonly ok: true; readonly spec: WorkspaceSpec; readonly instance: string }
	| { readonly ok: false; readonly reason: string };

/** Checks the choices and builds the spec. */
export const planWorkspace = (
	view: ViewState,
	choices: WorkspaceChoices,
	live: (device: DeviceId) => boolean
): WorkspacePlan => {
	const project = choices.project ? view.tables.project.get(choices.project) : undefined;
	if (!project) return { ok: false, reason: t("Pick a project.") };
	if (!project.repo)
		return {
			ok: false,
			reason: t("{{project_name}} has no remote to clone.", { project_name: project.name }),
		};
	const device = choices.device ? view.tables.device.get(choices.device) : undefined;
	if (!device) return { ok: false, reason: t("Pick a device.") };
	const where = deviceName(view, device.id);
	if (!device.workspace_host)
		return { ok: false, reason: t("{{where}} has no Incus.", { where: where }) };
	if (!live(device.id))
		return { ok: false, reason: t("Connect to {{where}} first.", { where: where }) };
	if (!makesWorkspaces(view, device.id)) {
		return { ok: false, reason: t("{{where}} doesn't make workspaces now.", { where: where }) };
	}
	const typed = choices.name.trim();
	const name = typed || defaultName(project.name);
	if (!NAME.test(name)) {
		return {
			ok: false,
			reason: t("Use lower-case letters, digits and hyphens for the name, up to 59 of them."),
		};
	}
	const instance = `mhq-${name}`;
	const taken = all(view, "workspace").some(
		(w) =>
			w.device === device.id &&
			w.location.type === "container" &&
			w.location.instance === instance
	);
	if (taken)
		return {
			ok: false,
			reason: t("{{instance}} already exists on {{where}}.", {
				instance: instance,
				where: where,
			}),
		};
	return {
		ok: true,
		instance,
		spec: {
			project: project.id,
			device: device.id,
			name: typed ? typed : null,
			start_at_boot: choices.startAtBoot,
		},
	};
};

/** The projects you can make a workspace for, by name. */
export const projectChoices = (view: ViewState): Option[] =>
	all(view, "project")
		.sort((a, b) => a.name.localeCompare(b.name))
		.map((p) => ({
			value: p.id,
			label: p.name,
			detail: p.repo ?? t("No remote to clone"),
			disabled: !p.repo,
		}));

/** The devices with Incus, by name. */
export const workspaceHosts = (view: ViewState): DeviceId[] =>
	all(view, "device")
		.filter((d) => d.workspace_host !== null)
		.map((d) => d.id)
		.sort((a, b) => deviceName(view, a).localeCompare(deviceName(view, b)));

/** The devices a new workspace can go on, with the ones not connected greyed out. */
export const hostChoices = (view: ViewState, live: (device: DeviceId) => boolean): Option[] =>
	workspaceHosts(view).map((id) => {
		const host = view.tables.device.get(id)?.workspace_host;
		const makes = makesWorkspaces(view, id);
		return {
			value: id,
			label: deviceName(view, id),
			detail: !live(id)
				? t("Not connected")
				: makes
					? t("Incus, on {{host__pools_join___________its_default_pool}}", {
							host__pools_join___________its_default_pool:
								host?.pools.join(", ") || "its default pool",
						})
					: t("Doesn't make workspaces now"),
			disabled: !live(id) || !makes,
		};
	});
