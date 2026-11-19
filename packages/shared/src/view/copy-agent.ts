// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a row's Copy entry offers for one agent: its folder's path, its
// branch, and the ID its CLI resumes the session by, each only when it
// has one. Every client's list copies the same things under the same
// labels.

import { t } from "../i18n";

import { sessionIdName } from "../agent-view/controls";
import type { Agent, Vendor } from "../bindings";

import type { ViewState } from "./replica";

/** One thing to copy, under its label. */
export type AgentCopy = { readonly label: string; readonly text: string };

/** What the agent's CLI calls its session ID, starting with a capital. */
export const sessionIdLabel = (vendor: Vendor): string => {
	const name = sessionIdName({ vendor });
	return name.charAt(0).toUpperCase() + name.slice(1);
};

/** What there is to copy for `agent`, in the menu's order; empty when nothing. */
export const agentCopies = (view: ViewState, agent: Agent): readonly AgentCopy[] => {
	const copies: AgentCopy[] = [];
	const path = view.tables.workspace.get(agent.workspace)?.location.path;
	if (path) copies.push({ label: t("Folder path"), text: path });
	if (agent.branch) copies.push({ label: t("Branch"), text: agent.branch });
	if (agent.vendor_session_id) {
		copies.push({ label: sessionIdLabel(agent.vendor), text: agent.vendor_session_id });
	}
	return copies;
};
