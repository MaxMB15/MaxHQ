// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether a Claude Code or Codex agent carries on once the usage limit that
// cut its turn off resets. Each agent has its own switch, and an agent
// without one follows its device's default for its vendor. A `maxd` from
// before the switch doesn't report the defaults.

import type { Agent, DeviceId } from "../bindings";
import { agentKind } from "../status/agent-kind";
import { inboxOf } from "../view/inbox";
import type { ViewState } from "../view/replica";

/** Whether a device's `maxd` has the switch, as its settings show. */
export const continuesAtLimit = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.continue_claude_at_limit !== undefined;

/** The device an agent runs on, when its workspace is known. */
const deviceOf = (view: ViewState, agent: Agent): DeviceId | undefined =>
	view.tables.workspace.get(agent.workspace)?.device;

/**
 * Whether the switch applies to an agent: a lead Claude Code or Codex agent
 * MaxHQ holds, on a `maxd` that has it. Cursor doesn't say when a limit
 * resets, an agent in your own terminal can't be sent anything, and a local
 * model has no limit to wait for.
 */
export const canContinueAtLimit = (view: ViewState, agent: Agent): boolean => {
	if (agent.parent !== null || agent.control !== "held") return false;
	const kind = agentKind(agent, view.tables.account);
	if (kind !== "claude" && kind !== "codex") return false;
	const device = deviceOf(view, agent);
	return device !== undefined && continuesAtLimit(view, device);
};

/** Whether an agent carries on after a limit: its own switch, or else its device's default. */
export const continuesAfterLimit = (view: ViewState, agent: Agent): boolean => {
	if (!canContinueAtLimit(view, agent)) return false;
	const own = inboxOf(agent).continue_at_limit;
	if (own !== undefined && own !== null) return own;
	const device = deviceOf(view, agent);
	const settings = device === undefined ? undefined : view.tables.device_settings.get(device);
	return agent.vendor === "claude"
		? settings?.continue_claude_at_limit === true
		: settings?.continue_codex_at_limit === true;
};
