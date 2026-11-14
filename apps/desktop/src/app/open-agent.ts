// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Opening an agent, from a sidebar click or a notification. It gets a pane,
// or its pane takes focus, and a sub-agent opens with its lead's panel.
// Looking at it marks it seen and clears its Woke marker, by the rule the
// phone shares.

import { askForRequest } from "@/agent-view/request";
import { type OpenOptions, panesOf } from "@/layout/panes";
import type { Agent, AgentId } from "@maxhq/shared/bindings";
import { slideOutOf } from "@maxhq/shared/ui/store";
import { lookAt } from "@maxhq/shared/view/seen";

import { childrenOf, needsYou } from "@maxhq/shared/view/select";
import { runsOf } from "@maxhq/shared/workflow/select";
import type { Services } from "./services";

/**
 * Marks the agent you're looking at seen when it's done and unread, and
 * clears its Woke marker. That's the agent whose pane has focus, or none
 * while the window is in the background.
 */
export const markSeenIfLooking = (services: Services, id: AgentId | null): void => {
	if (id !== null) lookAt(services, id, false);
};

/**
 * Opens an agent's pane or focuses it, selects the agent, opens its
 * slide-out when it has one, marks it seen and clears its Woke
 * marker.
 */
export const openAgent = (services: Services, id: AgentId, options?: OpenOptions): void => {
	// The pane opens first, so a split lands before the selection asks for a tab.
	panesOf(services).open(id, options);
	const view = services.view.getState().view;
	const agent = view.tables.agent.get(id);
	if (agent?.parent) {
		// One change, so the panes never see the lead selected on the way.
		const slideOut = slideOutOf(services.ui.getState(), agent.parent);
		services.ui.setState({ selectedAgent: id, slideOut, mode: "projects" });
	} else {
		// A lead with a run but no sub-agents has a slide-out too, which
		// opens on the run, the tab with something in it.
		const children = childrenOf(view, id).length;
		const slides = children > 0 || runsOf(view, id).length > 0;
		services.ui.getState().selectAgent(id, slides, children > 0 ? "agents" : "workflow");
	}
	// Opening it is looking at it, whatever the window's focus says.
	lookAt(services, id, true);
};

/**
 * Opens the next agent waiting on you, going round from the one you're
 * looking at, the longest waiting first, and goes to the card it waits on.
 * It passes over the agents `skip` names. Returns whether any agent waits.
 */
export const openNextWaiting = (
	services: Services,
	skip: (agent: Agent) => boolean = () => false
): boolean => {
	const waiting = needsYou(services.view.getState().view);
	const selected = services.ui.getState().selectedAgent;
	const at = waiting.findIndex((item) => item.agent.id === selected);
	const next = [...waiting.slice(at + 1), ...waiting.slice(0, at + 1)].find(
		(item) => !skip(item.agent)
	)?.agent;
	if (!next) return false;
	openAgent(services, next.id);
	askForRequest(next.id, next.activity.state === "needs_input" ? next.activity.attention : null);
	return true;
};
