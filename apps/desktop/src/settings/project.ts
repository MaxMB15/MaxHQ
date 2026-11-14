// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sends a project's first settings. A device that keeps nothing for the
// project gets them before a workspace is made or an agent starts there, so
// its agents can commit before anyone opens the settings. The model itself
// lives in the shared package.

import type { Services } from "@/app/services";
import type { AgentSpec, DeviceId, ProjectId } from "@maxhq/shared/bindings";
import { firstSettingsOn, keptSettingsOn } from "@maxhq/shared/settings/project";
import { isMade } from "@maxhq/shared/workspaces/state";

import { failureOf } from "./result";
import { t as translate } from "@maxhq/shared/i18n";

export * from "@maxhq/shared/settings/project";

/** What sending the first settings needs: the source, and the view as it is now. */
export type SettingsSender = Pick<Services, "source" | "view">;

/**
 * Sends a device the project's first settings when it keeps none, so agents
 * in its containers commit as this Mac's git identity. What comes next goes
 * ahead either way: git with no identity sends nothing, and a send that
 * fails is only logged.
 */
export const sendFirstSettings = async (
	services: SettingsSender,
	project: ProjectId,
	device: DeviceId
): Promise<void> => {
	const view = () => services.view.getState().view;
	// A device that keeps settings already needs nothing, so git isn't asked.
	if (keptSettingsOn(view(), project, device)) return;
	try {
		const identity = await services.source.gitIdentity();
		// Checked again, since the device may have reported some while git answered.
		const settings = firstSettingsOn(view(), project, device, identity);
		if (!settings) return;
		const failure = failureOf(await services.source.setProjectSettings(settings));
		if (failure) console.warn(translate("Couldn't send the project's first settings"), failure);
	} catch (error) {
		console.warn(translate("Couldn't send the project's first settings"), error);
	}
};

/**
 * The same for the workspace a new agent starts in, when MaxHQ made it for a
 * project. Other places have no project settings to put an author in.
 */
export const sendFirstSettingsFor = async (
	services: SettingsSender,
	spec: AgentSpec
): Promise<void> => {
	if (spec.place.type !== "workspace") return;
	const workspace = services.view.getState().view.tables.workspace.get(spec.place.workspace);
	if (!workspace || !isMade(workspace) || workspace.project === null) return;
	await sendFirstSettings(services, workspace.project, workspace.device);
};
