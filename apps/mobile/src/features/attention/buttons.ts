// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The buttons under a prompt. An approval puts its first yes first and its
// no last, as the desktop card does. When it takes a reason, its no moves
// to a field of its own, a box for the reason with the no on its end. A
// question lists its options in order.

import { approvalChoices } from "@maxhq/shared/agent-view/answer";
import type { AttentionOption } from "@maxhq/shared/bindings";

/** How a button looks. */
export type Tone = "primary" | "secondary" | "danger";

/** One button and the option it sends. */
export type AnswerButton = { readonly option: AttentionOption; readonly tone: Tone };

/** A prompt's buttons, and the no that sits on the reason field. */
export type Answers = {
	readonly buttons: readonly AnswerButton[];
	/** Sent with the reason typed beside it, when the prompt takes one. */
	readonly deny: AttentionOption | null;
};

/** The buttons for a prompt's options, in the order they show. */
export const answerButtons = (
	options: readonly AttentionOption[],
	permission: boolean,
	acceptsText = false
): Answers => {
	if (!permission) {
		return { buttons: options.map((option) => ({ option, tone: "secondary" })), deny: null };
	}
	const { primary, others, deny } = approvalChoices(options);
	const attached = acceptsText && deny ? deny : null;
	return {
		buttons: [
			...(primary ? [{ option: primary, tone: "primary" as const }] : []),
			...others.map((option) => ({ option, tone: "secondary" as const })),
			...(deny && !attached ? [{ option: deny, tone: "danger" as const }] : []),
		],
		deny: attached,
	};
};
