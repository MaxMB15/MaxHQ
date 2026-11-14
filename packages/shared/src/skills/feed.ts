// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the composer's `$` menu lists from: the last scan of the agent's
// workspace, asked for only once the menu opens, or why there's nothing.

import { t } from "../i18n";

import { useMemo } from "react";

import type { AgentId, AgentPlace, DeviceId, EffectiveSkill, Vendor } from "../bindings";
import type { DataSource } from "../data/source";
import type { ViewState } from "../view/replica";

import { offersSkills } from "./library";
import { type SkillScan, useSkillScan } from "./scan";

/** The skills a `$` menu picks from. */
export type SkillFeed = {
	/** The CLI whose skills the menu lists. */
	readonly cli: Vendor;
	/** The skills the last scan found, or null before one. */
	readonly skills: readonly EffectiveSkill[] | null;
	/** True while the first scan runs. */
	readonly loading: boolean;
	/** Why there's nothing to list, such as a device that doesn't scan. */
	readonly notice: string | null;
};

/** Where an agent's scan looks: its workspace, on the device that holds it. */
export const agentScanPlace = (
	view: ViewState,
	agent: AgentId
): { readonly device: DeviceId; readonly place: AgentPlace } | null => {
	const found = view.tables.agent.get(agent);
	const workspace = found && view.tables.workspace.get(found.workspace);
	if (!workspace) return null;
	return { device: workspace.device, place: { type: "workspace", workspace: workspace.id } };
};

/** A feed from a scan, or from a device that can't scan. */
export const feedOf = (cli: Vendor, offered: boolean, scan: SkillScan | undefined): SkillFeed => {
	if (!offered) {
		return {
			cli,
			skills: null,
			loading: false,
			notice: t("This agent's device doesn't list skills"),
		};
	}
	const skills = scan?.skills ?? null;
	const failed = scan?.state === "failed" && skills === null;
	return {
		cli,
		skills,
		loading: skills === null && !failed,
		notice: failed ? t("Couldn't read this agent's skills") : null,
	};
};

/**
 * The feed for an agent's `$` menu. It scans the agent's workspace once
 * `open`, and keeps the answer for the next time the menu opens.
 */
export const useSkillFeed = (
	source: Pick<DataSource, "scanSkills">,
	view: ViewState,
	agent: AgentId,
	open: boolean
): SkillFeed => {
	const at = agentScanPlace(view, agent);
	const cli = view.tables.agent.get(agent)?.vendor ?? "claude";
	const device = at?.device ?? null;
	const offered = device !== null && offersSkills(view, device);
	const workspace = at?.place.type === "workspace" ? at.place.workspace : null;
	const place = useMemo(
		(): AgentPlace | null =>
			open && offered && workspace ? { type: "workspace", workspace } : null,
		[open, offered, workspace]
	);
	const scan = useSkillScan(source, place ? device : null, place);
	return feedOf(cli, offered, scan);
};
