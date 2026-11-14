// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { Agent, DeviceId } from "../bindings";
import type { ViewState } from "../view/replica";

import type { AgentRef } from "./draft";

/**
 * An agent as a pill, on the device its workspace is on, or on `fallback`
 * while the view doesn't know the workspace.
 */
export const agentPillOf = (view: ViewState, agent: Agent, fallback: DeviceId): AgentRef => ({
	type: "agent",
	agent: agent.id,
	title: agent.title,
	vendor: agent.vendor,
	device: view.tables.workspace.get(agent.workspace)?.device ?? fallback,
	excerpt: null,
});
