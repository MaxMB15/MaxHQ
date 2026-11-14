// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which prompts are new while the app is open, so the phone can buzz and
// show a banner for one. A prompt is new when an agent starts waiting on
// it after its device has synced. What was already waiting when the app
// opened, or came back with a device after a reconnect, stays quiet.

import { t } from "../i18n";

import type { Agent, AgentId, AttentionEvent, AttentionId, Timestamp } from "../bindings";
import { showHidden } from "../status/format";
import { agentSynced } from "../status/reach";
import { all, type ViewState } from "../view/replica";

/** A prompt that just came in. */
export type Arrival = {
	readonly agent: Agent;
	readonly attention: AttentionId;
	/** The prompt, when the view has it. */
	readonly event: AttentionEvent | undefined;
};

/** The prompts agents wait on now. */
export const waitingPrompts = (view: ViewState): ReadonlySet<AttentionId> =>
	new Set(
		all(view, "agent").flatMap((agent) =>
			agent.activity.state === "needs_input" ? [agent.activity.attention] : []
		)
	);

/**
 * The prompts new since `known`, oldest first, and the prompts known
 * after. One whose device hasn't synced is part of what the device sends
 * as its link comes up, so it becomes known without counting as new.
 */
export const newPrompts = (
	known: ReadonlySet<AttentionId>,
	view: ViewState,
	now: Timestamp
): { readonly arrived: Arrival[]; readonly known: ReadonlySet<AttentionId> } => {
	const arrived = all(view, "agent")
		.flatMap((agent): Arrival[] => {
			if (agent.activity.state !== "needs_input") return [];
			const { attention } = agent.activity;
			if (known.has(attention) || !agentSynced(view, agent.id, now)) return [];
			return [{ agent, attention, event: view.tables.attention.get(attention) }];
		})
		.sort(
			(a, b) =>
				(a.event?.created_at ?? a.agent.last_activity_at) -
				(b.event?.created_at ?? b.agent.last_activity_at)
		);
	return { arrived, known: waitingPrompts(view) };
};

/** The newest arrival for an agent other than the one on screen, for the banner. */
export const bannerArrival = (arrived: readonly Arrival[], shown: AgentId | null): Arrival | null =>
	arrived.filter((each) => each.agent.id !== shown).at(-1) ?? null;

/** What the banner says of a prompt: the agent, and what it asks. */
export const arrivalText = (
	arrival: Arrival
): { readonly title: string; readonly body: string } => {
	const ask =
		arrival.event?.kind === "permission"
			? t("Asks to run a tool")
			: arrival.event?.kind === "question"
				? t("Asks you a question")
				: arrival.event?.kind === "error"
					? t("Stopped on an error")
					: t("Needs input");
	return { title: arrival.agent.title, body: showHidden(arrival.event?.summary || ask) };
};

/** Whether a banner's prompt is still open. */
export const stillWaiting = (view: ViewState, arrival: Arrival): boolean => {
	const agent = view.tables.agent.get(arrival.agent.id);
	return (
		agent?.activity.state === "needs_input" && agent.activity.attention === arrival.attention
	);
};
