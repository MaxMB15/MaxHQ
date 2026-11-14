// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Running inbox changes from the menus and the shortcuts. A menu acts on
// the agents it was opened for; a shortcut acts on the agents you picked,
// or else on the one you're looking at. A change made to picked agents
// ends the pick. Settling the one you're looking at goes on to the next
// agent waiting on you.

import { openNextWaiting } from "@/app/open-agent";
import type { Services } from "@/app/services";
import type { Agent, AgentId } from "@maxhq/shared/bindings";
import { changeInbox, undoNotice } from "@maxhq/shared/ui/inbox-notice";
import {
	type InboxChoice,
	type InboxMenu,
	type InboxStep,
	inboxMenu,
	inboxTargets,
} from "@maxhq/shared/view/inbox-actions";

import { noticesOf } from "./notices";

/**
 * Makes a change and leaves its notice. `text` words the notice when the
 * change alone can't.
 */
export const runSteps = (
	services: Services,
	steps: readonly InboxStep[],
	text?: (done: readonly Agent[]) => string
): void => {
	const picked = services.ui.getState().picked;
	if (steps.some((step) => step.agents.some((id) => picked.has(id)))) {
		services.ui.getState().clearPicked();
	}
	void changeInbox(noticesOf(services), steps, text);
};

/** Runs a menu choice, when there is one. */
export const runChoice = (
	services: Services,
	choice: InboxChoice | null,
	text?: (done: readonly Agent[]) => string
): void => {
	if (choice) runSteps(services, choice.steps, text);
};

/**
 * The agents a shortcut acts on: the ones you picked, or else the one
 * you're looking at. Looking at a sub-agent means its lead, which the
 * inbox moves with.
 */
export const chosenAgents = (services: Services): AgentId[] => {
	const { picked, selectedAgent } = services.ui.getState();
	if (picked.size > 0) return [...picked];
	if (selectedAgent === null) return [];
	const agent = services.view.getState().view.tables.agent.get(selectedAgent);
	return agent ? [agent.parent ?? agent.id] : [];
};

// Runs the choice for the chosen agents, and returns the agents it changes.
const runOnChosen = (
	services: Services,
	pick: (menu: InboxMenu) => InboxChoice | null
): AgentId[] => {
	const view = services.view.getState().view;
	const targets = inboxTargets(view, chosenAgents(services));
	if (targets.length === 0) return [];
	const choice = pick(inboxMenu(view, targets, Date.now()));
	runChoice(services, choice);
	return choice ? choice.steps.flatMap((step) => step.agents) : [];
};

/** Settles the chosen agents that aren't settled yet, and returns them. */
export const settleChosen = (services: Services): AgentId[] =>
	runOnChosen(services, (menu) =>
		menu.settle?.steps[0]?.change.change === "settle" ? menu.settle : null
	);

/**
 * Settles the chosen agents. With none picked, that's the one you're
 * looking at, and then the next agent waiting on you opens as ⌘J opens
 * it. Returns whether one opened, and stays when no other waits.
 */
export const settleAndMoveOn = (services: Services): boolean => {
	if (services.ui.getState().picked.size > 0) {
		settleChosen(services);
		return false;
	}
	const settled = new Set(settleChosen(services));
	if (settled.size === 0) return false;
	// A settled lead takes its sub-agents to the shelf with it.
	return openNextWaiting(
		services,
		(agent) => settled.has(agent.id) || (agent.parent !== null && settled.has(agent.parent))
	);
};

/** Pins the chosen agents, or unpins them when every one is pinned. */
export const togglePinChosen = (services: Services): void => {
	runOnChosen(services, (menu) => menu.pin);
};

/** Whether a notice with an Undo shows. */
export const canUndoInbox = (services: Services): boolean =>
	noticesOf(services).store.getState().notice !== null;

/** Undoes the change the notice shows. */
export const undoInbox = (services: Services): void => {
	undoNotice(noticesOf(services));
};
