// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the compose box sends. Text only for now. A busy agent gets it once
// its turn ends, like Enter on the desktop, and an empty box stops the turn
// instead. While a prompt waits, Send now skips it and sends the message as
// the next turn. A session started in a terminal gets no box: MaxHQ only
// follows it until it continues here.

import { t } from "@maxhq/shared/i18n";

import type { Activity, Agent, AgentId, SendMode, UserInput } from "@maxhq/shared/bindings";
import { attempt, canStop, failureOf, nowMode, turnOf } from "@maxhq/shared/composer/send";
import type { ActionResult } from "@maxhq/shared/data/source";
import type { SendGate } from "@maxhq/shared/settings/auto-mode";

import type { Tone } from "../attention/buttons";

/** The mode the compose box sends with. */
export const SEND_MODE: SendMode = "queue";

/** The input for what you typed, or null when there's nothing to send. */
export const inputOf = (text: string): UserInput | null => {
	const trimmed = text.trim();
	return trimmed ? { text: trimmed, attachments: [] } : null;
};

/** Whether MaxHQ runs the agent, so it takes messages from here at all. */
export const hasComposer = (agent: Agent): boolean => agent.control !== "observed";

/**
 * How Continue in MaxHQ looks. While the terminal runs the session it's a
 * quiet button, as the desktop draws it, since a dimmed blue one still
 * looks like it can be pressed.
 */
export const continueTone = (ended: boolean): Tone => (ended ? "primary" : "secondary");

/** Whether the agent can take a message now. */
export const canSend = (activity: Activity): boolean => turnOf(activity) !== "offline";

/** The send button's word and the hint beside it, as the turn stands. */
export const sendLabel = (activity: Activity): { label: string; hint: string | null } => {
	switch (turnOf(activity)) {
		case "busy":
		case "prompt":
			return { label: t("Queue"), hint: t("Sends when the turn ends") };
		case "waiting":
			return { label: t("Queue"), hint: t("Sends when the limit resets") };
		default:
			return { label: t("Send"), hint: null };
	}
};

/** Whether the send button stops the turn instead: one runs and the box is empty. */
export const stopsTurn = (activity: Activity, text: string): boolean =>
	canStop(turnOf(activity)) && inputOf(text) === null;

/**
 * Whether Send now shows beside Queue: a prompt waits and you typed
 * something, which goes as the next turn once maxd dismisses the prompt.
 */
export const sendsNow = (activity: Activity, text: string): boolean =>
	nowMode(turnOf(activity)) === "interrupt" && inputOf(text) !== null;

/** Where the box keeps what you typed for an agent, so it stays with that agent. */
export const draftKey = (agent: AgentId): string => `draft:${agent}`;

/** Whether a message went, and what to show under the box. */
export type Sent = { readonly sent: boolean; readonly failure: string | null };

/**
 * Sends with `send` once `agree` lets it, as an Auto you didn't pick asks
 * first. A message that went still shows why a change didn't go through.
 */
export const sendAgreed = async (
	agree: () => Promise<SendGate>,
	send: () => Promise<ActionResult>
): Promise<Sent> => {
	const gate = await agree();
	if (!gate.go) return { sent: false, failure: gate.failure };
	const failure = failureOf(await attempt(send), "send");
	return failure === null ? { sent: true, failure: gate.failure } : { sent: false, failure };
};
