// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the compose box sends. The send button works as Enter does on the
// desktop: a busy agent gets the message as this phone
// picked, queued for when its turn ends or steered into the turn, and an
// empty box stops the turn instead. Once you type, the button beside it
// does what Command-Enter does: the other of Queue and Steer, or Send now,
// which skips a waiting prompt and sends the message as the next turn. A
// session started in a terminal gets no box: MaxHQ only follows it until it
// continues here.

import { t } from "@maxhq/shared/i18n";

import { SKIP_WORDS } from "@maxhq/shared/agent-view/dismiss";
import type { Activity, Agent, AgentId, SendMode } from "@maxhq/shared/bindings";
import {
	BUSY_SEND_WORDS,
	type BusySend,
	canStop,
	sendKeys,
	turnOf,
} from "@maxhq/shared/composer/send";

import type { Tone } from "../attention/buttons";

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

/** How the send button sends, as the turn stands and `pick` says. */
export const sendMode = (activity: Activity, pick: BusySend): SendMode =>
	sendKeys(turnOf(activity), pick).enter;

/** The send button's word and the hint beside it, as the turn stands and `pick` says. */
export const sendLabel = (
	activity: Activity,
	pick: BusySend
): { label: string; hint: string | null } => {
	switch (turnOf(activity)) {
		case "busy":
			return pick === "steer"
				? { label: t("Steer"), hint: t("The agent reads it between steps") }
				: { label: t("Queue"), hint: t("Sends when the turn ends") };
		case "prompt":
			return { label: t("Queue"), hint: t("Sends when the turn ends") };
		case "waiting":
			return { label: t("Queue"), hint: t("Sends when the limit resets") };
		default:
			return { label: t("Send"), hint: null };
	}
};

/**
 * Whether the send button stops the turn instead: one runs and the box is
 * empty, as `filled` says it isn't.
 */
export const stopsTurn = (activity: Activity, filled: boolean): boolean =>
	canStop(turnOf(activity)) && !filled;

/** The button beside the send button: how it sends, its word and what it does. */
export type OtherSend = { readonly mode: SendMode; readonly label: string; readonly hint: string };

/**
 * The button beside the send button, once the box is `filled`: while a turn
 * runs, the one of Queue and Steer you didn't pick, and while a prompt
 * waits, Send now, which goes as the next turn once maxd dismisses the
 * prompt. Null otherwise.
 */
export const otherSend = (
	activity: Activity,
	pick: BusySend,
	filled: boolean
): OtherSend | null => {
	const mode = sendKeys(turnOf(activity), pick).modified;
	if (mode === null || !filled) return null;
	return mode === "interrupt"
		? { mode, label: SKIP_WORDS.sendNow, hint: SKIP_WORDS.sendNowTip }
		: { mode, label: BUSY_SEND_WORDS[mode].label, hint: BUSY_SEND_WORDS[mode].line };
};

/** Where the box keeps what you typed for an agent, so it stays with that agent. */
export const draftKey = (agent: AgentId): string => `draft:${agent}`;
