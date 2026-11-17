// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a device's card on a project's settings page says, as the desktop's
// does: whether the device is there to change them, and for each setting
// a maxd may be too old for, the line under it and whether it takes a
// change now.

import { t } from "@maxhq/shared/i18n";
import { settleDaysText } from "@maxhq/shared/settings/inbox";
import type { ProjectCapabilities } from "@maxhq/shared/settings/project";

/** The line under a setting, and whether it takes a change now. */
export type ProjectRow = { readonly note: string; readonly editable: boolean };

/** The card's line, and the settings an old maxd can't keep. */
export type ProjectRows = {
	readonly state: string;
	readonly settle: ProjectRow;
	readonly pulls: ProjectRow;
	readonly workflows: ProjectRow;
	readonly memory: ProjectRow;
};

/** What the card knows about the device. */
export type ProjectFacts = {
	/** The device's name. */
	readonly name: string;
	/** Whether its maxd answers now. */
	readonly live: boolean;
	readonly capabilities: ProjectCapabilities;
	/** The idle days the device settles agents after, which an empty field leaves. */
	readonly deviceDays: number;
	/** The workflow variables you allowed for every agent here, in words, or null. */
	readonly allowed: string | null;
};

/** The card's line and each setting's, as the desktop words them. */
export const projectRows = ({
	name,
	live,
	capabilities,
	deviceDays,
	allowed,
}: ProjectFacts): ProjectRows => {
	const old = t("{{name}}'s maxd is too old for this. Update it first.", { name: name });
	const row = (kept: boolean, note: string): ProjectRow => ({
		note: kept ? note : old,
		editable: live && kept,
	});
	return {
		state: live
			? t("Kept on the device")
			: t("Connect to {{name}} to change it", { name: name }),
		settle: row(
			capabilities.settle,
			t(
				"Days with no activity before an agent in this project settles. Leave it empty to use {{name}}'s, which is {{value2}}. 0 never settles them.",
				{ name: name, value2: settleDaysText(deviceDays).toLowerCase() }
			)
		),
		pulls: row(
			capabilities.pulls,
			t(
				"Once a PR linked to an agent in this project merges, the agent settles. Off leaves it in the inbox."
			)
		),
		workflows: row(
			capabilities.workflows,
			t(
				"Claude agents MaxHQ starts here can get the variables this project's workflow names, once you allow them on the card the agent shows. Off leaves only the agent's own name.{{value1}}",
				{ value1: allowed ? ` ${allowed}` : "" }
			)
		),
		memory: row(
			capabilities.memory,
			t(
				"A soft limit for each container workspace of this project. Past it, {{name}} reclaims the container's memory first. Leave it empty to use the host's.",
				{ name: name }
			)
		),
	};
};
