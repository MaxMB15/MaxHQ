// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's workspaces as every client shows them on its page: the ones
// MaxHQ made, whether it makes more now, the lines around them, and what a
// delete the device refused or a container's boot setting says.

import { formatNumber, t } from "../i18n";

import type { DeviceId, Timestamp } from "../bindings";
import type { ActionResult } from "../data/source";
import { offersSnapshots } from "../hosts/snapshots";
import { all, type ViewState } from "../view/replica";
import { deviceName } from "../view/select";
import {
	isLive,
	isMade,
	type MadeWorkspace,
	madeWorkspaces,
	makesWorkspaces,
	type WorkspaceAction,
} from "./state";

/** A device's workspaces section. */
export type DeviceWorkspacesView = {
	readonly made: MadeWorkspace[];
	/** Whether it has Incus and its `maxd` makes workspaces. */
	readonly makes: boolean;
	readonly live: boolean;
	/** The line under the title. */
	readonly hint: string;
	/** The line beside the buttons: the containers left alone, or how to make one. */
	readonly footer: string;
};

/** What a device's workspaces section shows, or null when it has no Incus and none were made. */
export const deviceWorkspaces = (
	view: ViewState,
	device: DeviceId,
	now: Timestamp
): DeviceWorkspacesView | null => {
	const name = deviceName(view, device);
	const made = madeWorkspaces(view, { device });
	const host = view.tables.device.get(device)?.workspace_host ?? null;
	if (!host && made.length === 0) return null;
	const others = all(view, "workspace").filter(
		(w) => w.device === device && w.location.type === "container" && !isMade(w)
	).length;
	const live = isLive(view, device, now);
	// A device whose `maxd` says it makes no workspaces gets no New workspace.
	const makes = host !== null && makesWorkspaces(view, device);
	return {
		made,
		makes,
		live,
		hint: !host
			? t("{{name}} has no Incus", { name: name })
			: makes
				? t("Containers MaxHQ makes and runs")
				: t("{{name}} doesn't make workspaces now", { name: name }),
		footer:
			others > 0
				? t(
						"MaxHQ didn't make the other {{countText}} containers here, so it leaves them alone.",
						{ count: others, countText: formatNumber(others) }
					)
				: live || !makes
					? ""
					: t("Connect to {{name}} to make one.", { name: name }),
	};
};

/** The device's reason when it refused a delete for work that isn't pushed, or null. */
export const deleteRefusal = (result: ActionResult): string | null =>
	result.status === "ok" && result.data.status === "refused" ? result.data.reason : null;

/** When a workspace's container starts, `where` being its device's name. */
export const bootLine = (workspace: MadeWorkspace, where: string): string =>
	workspace.managed.start_at_boot
		? t("Starts when {{where}} boots", { where: where })
		: t("Starts only when you start it");

/**
 * Whether a workspace's card offers its snapshots, given what you can do
 * to it now. Snapshots need the device to answer and a container that's
 * there to start or stop, so not one whose storage is locked or that's
 * being made.
 */
export const showsSnapshots = (
	view: ViewState,
	workspace: MadeWorkspace,
	actions: readonly WorkspaceAction[]
): boolean =>
	offersSnapshots(view, workspace.device) &&
	actions.some((action) => action === "start" || action === "stop");
