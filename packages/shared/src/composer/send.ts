// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// When a message goes out and how. An idle agent takes it at once. While a
// turn runs, Enter queues it for when the turn ends and Steer sends it into
// the turn. While a prompt waits for an answer, Enter queues it too, and
// Send now dismisses what waits, stops the turn and sends it as the next.

import { t } from "../i18n";

import type { ActionResult } from "../data/source";
import type { Activity, CommandError, SendMode, TimelineItem } from "../bindings";
import { excerptIds, labeledText } from "../pills/draft";
import { formatClock, sentence } from "../status/format";

/** Where an agent's turn is, as far as sending goes. */
export type Turn =
	/** Takes a message at once. */
	| "idle"
	/** Working on a turn, which a message can join or wait for. */
	| "busy"
	/** Blocked on a permission prompt or a question, so a message waits. */
	| "prompt"
	/** At a usage limit, so a message waits for the reset. */
	| "waiting"
	/** Out of reach, so nothing can be sent. */
	| "offline";

/** The turn an agent's activity puts it in. */
export const turnOf = (activity: Activity): Turn => {
	switch (activity.state) {
		case "busy":
		case "busy_in_subagents":
			return "busy";
		case "needs_input":
			return "prompt";
		case "waiting_for_limit_reset":
			return "waiting";
		case "not_accessible":
			return "offline";
		default:
			return "idle";
	}
};

/** Whether Stop has a turn to stop. */
export const canStop = (turn: Turn): boolean => turn === "busy" || turn === "prompt";

/**
 * How a message goes at once rather than wait: Steer into the running turn,
 * or Send now past the prompts waiting on you, which maxd dismisses as it
 * stops the turn. Null when nothing waits to be skipped.
 */
export const nowMode = (turn: Turn): Extract<SendMode, "steer" | "interrupt"> | null =>
	turn === "busy" ? "steer" : turn === "prompt" ? "interrupt" : null;

/** What the empty composer says. */
export const placeholderFor = (activity: Activity, agentTitle: string, device: string): string => {
	switch (activity.state) {
		case "needs_input":
			return t("Queue a message for after this turn");
		case "waiting_for_limit_reset":
			return activity.resets_at === null
				? t("Queue a message for when the limit resets")
				: t("Queue a message for {{formatClock_activity_resets_at}}", {
						formatClock_activity_resets_at: formatClock(activity.resets_at),
					});
		case "not_accessible":
			return t("Can't reach {{device}} right now", { device: device });
		default:
			return t("Message {{agentTitle}} on {{device}}", {
				agentTitle: agentTitle,
				device: device,
			});
	}
};

/** A message you sent that waits for the turn to end. */
export type QueuedMessage = Extract<TimelineItem, { kind: "user_message" }>;

/** The messages that still wait, oldest first. */
export const waitingMessages = (
	items: readonly TimelineItem[],
	hidden: ReadonlySet<string>
): QueuedMessage[] =>
	items
		.filter(
			(item): item is QueuedMessage =>
				item.kind === "user_message" && item.delivery === "queued" && !hidden.has(item.id)
		)
		.sort((a, b) => a.ordinal - b.ordinal);

/** A waiting message as its bubble shows it. */
export type QueuedLook = {
	/** The text, with each file and quote it names in words. */
	readonly text: string;
	/** How many files you added to it. */
	readonly files: number;
};

/** How a waiting message reads in its bubble. */
export const queuedLook = (message: QueuedMessage): QueuedLook => {
	// An agent's excerpt goes along as an attachment, but you didn't add it.
	const excerpts = excerptIds(message.refs);
	return {
		text: labeledText(
			{
				text: message.text,
				attachments: message.attachments.map((ref) => ref.id),
				refs: message.refs,
			},
			(id) => message.attachments.find((ref) => ref.id === id)?.name
		),
		files: message.attachments.filter((ref) => !excerpts.has(ref.id)).length,
	};
};

/** Runs an action and turns a rejection into a failed result. */
export const attempt = (action: () => Promise<ActionResult>): Promise<ActionResult> =>
	action().catch((error: unknown): ActionResult => ({
		status: "error",
		error: {
			kind: "failed",
			message: error instanceof Error ? error.message : String(error),
		},
	}));

/** What the composer asked maxd to do, which its failure text names. */
export type ComposerAction =
	"send" | "steer" | "unqueue" | "stop" | "model" | "effort" | "permission";

const TRIED: Record<ComposerAction, string> = {
	get send() {
		return t("send the message");
	},
	get steer() {
		return t("steer the turn");
	},
	get unqueue() {
		return t("take the message back");
	},
	get stop() {
		return t("stop the turn");
	},
	get model() {
		return t("change the model");
	},
	get effort() {
		return t("change the effort level");
	},
	get permission() {
		return t("change the permission mode");
	},
};

const commandErrorText = (error: CommandError): string => {
	switch (error.kind) {
		case "not_found":
			return t("{{sentence_error_what}} isn't there anymore", {
				sentence_error_what: sentence(error.what),
			});
		case "not_connected":
			return t("{{error_device}} isn't connected", { error_device: error.device });
		case "failed":
			return sentence(error.message);
	}
};

const reasonOf = (result: ActionResult): string | null => {
	if (result.status === "error") return commandErrorText(result.error);
	switch (result.data.status) {
		case "refused":
			return sentence(result.data.reason);
		case "unknown_agent":
			return t("MaxHQ no longer knows this agent");
		default:
			return null;
	}
};

/**
 * Why an action didn't go through, or null when it did. On its own it's
 * the reason alone, such as "Mac isn't connected", for a chip. With the
 * action it's a sentence for the error line that says what didn't happen.
 */
export const failureOf = (result: ActionResult, action?: ComposerAction): string | null => {
	const reason = reasonOf(result);
	if (reason === null || action === undefined) return reason;
	return reason === ""
		? t("Couldn't {{TRIED_action}}.", { TRIED_action: TRIED[action] })
		: t("Couldn't {{TRIED_action}}. {{reason}}.", {
				TRIED_action: TRIED[action],
				reason: reason,
			});
};
