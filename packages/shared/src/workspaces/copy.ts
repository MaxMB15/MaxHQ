// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Copying a project to another device: which checkout it copies from, the
// devices that can take it, and the target to send, or why there isn't one
// yet. A copy lands in a new container or clones into a folder on the
// device's host, and the ignored files you tick follow it there.

import { t } from "../i18n";

import type { DeviceId, ProjectId, Workspace, WorkspaceId } from "../bindings";
import type { CopyTarget } from "../data/source";
import type { Option } from "../new-agent/spec";
import { all, type ViewState } from "../view/replica";
import { deviceName } from "../view/select";

import { canCopy, defaultName, makesWorkspaces, planWorkspace } from "./state";

/** What you pick for a copy. */
export type CopyChoices = {
	readonly from: WorkspaceId | null;
	readonly device: DeviceId | null;
	readonly kind: CopyTarget["type"];
	/** The container's name after `mhq-`, or empty for the project's name. */
	readonly container: string;
	/** The folder the clone goes in, as an absolute path on the device. */
	readonly parent: string;
	/** The clone's own folder inside the parent. */
	readonly folder: string;
};

/** The copy to send, or why there isn't one yet. */
export type CopyPlan =
	| {
			readonly ok: true;
			readonly target: CopyTarget;
			readonly project: ProjectId;
			/** Where it lands, such as "mhq-comet" or "/home/dev/code/comet". */
			readonly where: string;
	  }
	| { readonly ok: false; readonly reason: string };

/** Where a workspace lives, in a few words. */
export const checkoutOf = (workspace: Workspace): string =>
	workspace.location.type === "container" ? workspace.location.instance : workspace.location.path;

/** The project's checkouts you can copy from, by device. */
export const copySources = (view: ViewState, project: ProjectId): Workspace[] =>
	all(view, "workspace")
		.filter((w) => w.project === project)
		.sort(
			(a, b) =>
				deviceName(view, a.device).localeCompare(deviceName(view, b.device)) ||
				checkoutOf(a).localeCompare(checkoutOf(b))
		);

/** The projects with a checkout to copy, by name, greyed out with no remote to clone. */
export const projectsToCopy = (view: ViewState): Option[] =>
	all(view, "project")
		.filter((p) => copySources(view, p.id).length > 0)
		.sort((a, b) => a.name.localeCompare(b.name))
		.map((p) => ({
			value: p.id,
			label: p.name,
			detail: p.repo ?? t("No remote to clone"),
			disabled: !p.repo,
		}));

/** The checkouts as picker entries. */
export const sourceChoices = (view: ViewState, project: ProjectId): Option[] =>
	copySources(view, project).map((w) => ({
		value: w.id,
		label: deviceName(view, w.device),
		detail: checkoutOf(w),
	}));

/** Whether a device makes containers, so a copy can go in a new one. */
export const takesContainers = (view: ViewState, device: DeviceId): boolean =>
	!!view.tables.device.get(device)?.workspace_host && makesWorkspaces(view, device);

/** The devices a copy can go to, with the ones that can't take it greyed out. */
export const targetChoices = (view: ViewState, live: (device: DeviceId) => boolean): Option[] =>
	all(view, "device")
		.map((d) => d.id)
		.sort((a, b) => deviceName(view, a).localeCompare(deviceName(view, b)))
		.map((id) => {
			const up = live(id);
			const takes = canCopy(view, id);
			return {
				value: id,
				label: deviceName(view, id),
				detail: !up
					? t("Not connected")
					: !takes
						? t("Its maxd can't take copies yet")
						: takesContainers(view, id)
							? t("Container or folder")
							: t("Folder"),
				disabled: !up || !takes,
			};
		});

/** The folder name a clone of the repo gets, as `git clone` would pick it. */
export const cloneFolderName = (repo: string): string =>
	repo
		.replace(/[\\/]+$/, "")
		.replace(/\.git$/, "")
		.split(/[/:\\]/)
		.filter(Boolean)
		.at(-1) ?? "";

/** The folder a workspace's checkout sits in, as a first guess for the parent. */
export const parentOf = (workspace: Workspace | undefined): string => {
	if (workspace?.location.type !== "folder") return "";
	const path = workspace.location.path.replace(/\/+$/, "");
	return path.slice(0, path.lastIndexOf("/")) || "/";
};

/** The choices a copy starts with: the first checkout and the first device that takes it. */
export const firstChoices = (
	view: ViewState,
	project: ProjectId,
	pick: { from?: WorkspaceId | null; device?: DeviceId | null },
	live: (device: DeviceId) => boolean
): CopyChoices => {
	const sources = copySources(view, project);
	const from = sources.find((w) => w.id === pick.from) ?? sources[0];
	const device =
		pick.device ??
		targetChoices(view, live).find((o) => !o.disabled && o.value !== from?.device)?.value ??
		null;
	const repo = view.tables.project.get(project)?.repo ?? "";
	return {
		from: from?.id ?? null,
		device,
		kind: device && takesContainers(view, device) ? "container" : "folder",
		container: "",
		parent: parentOf(from),
		folder: cloneFolderName(repo),
	};
};

/** A folder path joined, with no doubled slash. */
const join = (parent: string, name: string) => `${parent.replace(/\/+$/, "")}/${name}`;

/** Checks the choices and builds the target. */
export const planCopy = (
	view: ViewState,
	choices: CopyChoices,
	live: (device: DeviceId) => boolean
): CopyPlan => {
	const from = choices.from ? view.tables.workspace.get(choices.from) : undefined;
	if (!from) return { ok: false, reason: t("Pick a checkout to copy from.") };
	const project = from.project ? view.tables.project.get(from.project) : undefined;
	if (!project) return { ok: false, reason: t("That checkout belongs to no project.") };
	if (!project.repo)
		return {
			ok: false,
			reason: t("{{project_name}} has no remote to clone.", { project_name: project.name }),
		};
	const device = choices.device ? view.tables.device.get(choices.device) : undefined;
	if (!device) return { ok: false, reason: t("Pick a device.") };
	const where = deviceName(view, device.id);
	if (!live(device.id))
		return { ok: false, reason: t("Connect to {{where}} first.", { where: where }) };
	if (!canCopy(view, device.id)) {
		return {
			ok: false,
			reason: t("The maxd on {{where}} can't take copies yet. Update it first.", {
				where: where,
			}),
		};
	}

	if (choices.kind === "container") {
		const plan = planWorkspace(
			view,
			{ project: project.id, device: device.id, name: choices.container, startAtBoot: false },
			live
		);
		if (!plan.ok) return plan;
		return {
			ok: true,
			project: project.id,
			where: plan.instance,
			target: { type: "container", device: device.id, name: plan.spec.name },
		};
	}

	const parent = choices.parent.trim();
	const name = choices.folder.trim();
	if (!parent.startsWith("/")) {
		return { ok: false, reason: t("Give the parent folder as a full path, starting with /.") };
	}
	if (!name || name === "." || name === ".." || name.includes("/")) {
		return { ok: false, reason: t("Name the folder the clone goes in, with no slashes.") };
	}
	const path = join(parent, name);
	const taken = all(view, "workspace").some(
		(w) => w.device === device.id && w.location.type === "folder" && w.location.path === path
	);
	if (taken)
		return {
			ok: false,
			reason: t("{{path}} is already a workspace on {{where}}.", {
				path: path,
				where: where,
			}),
		};
	return {
		ok: true,
		project: project.id,
		where: path,
		target: { type: "folder", device: device.id, parent, name },
	};
};

/**
 * The workspace a copy made, once the device reports it: a new container
 * for the project, or the folder at the planned path. `before` holds the
 * workspaces there were when the copy started.
 */
export const copyLanded = (
	view: ViewState,
	plan: Extract<CopyPlan, { ok: true }>,
	before: ReadonlySet<WorkspaceId>
): Workspace | null =>
	all(view, "workspace").find(
		(w) =>
			!before.has(w.id) &&
			w.device === plan.target.device &&
			(plan.target.type === "container"
				? w.location.type === "container" && w.project === plan.project
				: w.location.type === "folder" && w.location.path === plan.where)
	) ?? null;

/** The container name a copy suggests when you leave it empty. */
export const containerHint = (view: ViewState, project: ProjectId): string =>
	defaultName(view.tables.project.get(project)?.name ?? "") || "name";
