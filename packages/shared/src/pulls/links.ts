// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The PRs linked to agents. An agent's `pulls` holds every link its
// device keeps, the ones you dismissed too, so a dismissed PR isn't linked
// again; the chip and everything else shows only the rest, as maxd's
// `shown` does. Repositories compare in lower case, as maxd keeps them.

import type { Agent, DeviceId, PullKey, PullLink } from "../bindings";
import { all, type ViewState } from "../view/replica";

/** Whether two repositories are the same, whatever their case. */
export const sameRepo = (a: string, b: string): boolean => a.toLowerCase() === b.toLowerCase();

/** Whether two keys name the same PR. */
export const samePull = (a: PullKey, b: PullKey): boolean =>
	a.number === b.number && sameRepo(a.repo, b.repo);

/** The links an agent shows, leaving out the ones you dismissed. */
export const shownPulls = (agent: Agent): PullLink[] =>
	(agent.pulls ?? []).filter((link) => link.source !== "dismissed");

/** The agents that show a link to the PR. */
export const agentsWithPull = (view: ViewState, key: PullKey): Agent[] =>
	all(view, "agent").filter((agent) => shownPulls(agent).some((link) => samePull(link.key, key)));

/** The devices running an agent that shows a link to the PR, each once. */
export const devicesWithPull = (view: ViewState, key: PullKey): DeviceId[] => {
	const devices = agentsWithPull(view, key).flatMap((agent) => {
		const device = view.tables.workspace.get(agent.workspace)?.device;
		return device ? [device] : [];
	});
	return [...new Set(devices)];
};
