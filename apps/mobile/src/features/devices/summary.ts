// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The short lines the devices list and its button show: how many devices
// are live, connecting or not working, how many agents each runs, and
// which devices Settings says the phone is paired with.

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

// How many device names "Paired with" shows before it counts the rest.
const NAMED = 3;

/** Such as "Mac, devbox, buildbox and 3 more", or null with no devices. */
export const pairedWith = (devices: readonly DeviceReach[]): string | null => {
	const names = devices.map((d) => d.name);
	const last = names.pop();
	if (last === undefined) return null;
	if (names.length === 0) return last;
	if (names.length < NAMED) return t("{{names}} and {{last}}", { names: names.join(", "), last });
	return t("{{names}} and {{count}} more", {
		names: names.slice(0, NAMED).join(", "),
		count: names.length + 1 - NAMED,
	});
};
