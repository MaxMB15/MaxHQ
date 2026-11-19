// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The short lines the devices list and its button show: how many devices
// are live, connecting or not working, and how many agents each runs.

import { t } from "@maxhq/shared/i18n";

import type { DeviceReach } from "@maxhq/shared/status/reach";
import { all, type ViewState } from "@maxhq/shared/view/replica";

/** Such as "3 live, 1 connecting, 2 not working". */
export const reachSummary = (devices: readonly DeviceReach[]): string => {
	const live = devices.filter((d) => d.look === "live").length;
	const connecting = devices.filter((d) => d.look === "connecting").length;
	const down = devices.length - live - connecting;
	return [
		live > 0 ? t("{{count}} live", { count: live }) : null,
		connecting > 0 ? t("{{count}} connecting", { count: connecting }) : null,
		down > 0 ? t("{{count}} not working", { count: down }) : null,
	]
		.filter((part) => part !== null)
		.join(", ");
};

/** Such as "4 agents, 1 needs you", or null when the device runs none. */
export const agentSummary = (view: ViewState, link: string): string | null => {
	const device = view.tables.device_link.get(link)?.device;
	if (!device) return null;
	const agents = all(view, "agent").filter(
		(agent) =>
			agent.parent === null && view.tables.workspace.get(agent.workspace)?.device === device
	);
	if (agents.length === 0) return null;
	const asking = agents.filter((agent) => agent.activity.state === "needs_input").length;
	return asking > 0
		? t("{{agents}}, {{count}} need you", {
				agents: t("{{count}} agents", { count: agents.length }),
				count: asking,
			})
		: t("{{count}} agents", { count: agents.length });
};
