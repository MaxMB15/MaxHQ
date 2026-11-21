// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What an attention card shows, from the inbox's event or from the
// approval or question in the agent's feed, which says more: the full
// command, each question with its options, and how it was answered or
// that it was dismissed.

import { t } from "@maxhq/shared/i18n";

import { decidedLine } from "@maxhq/shared/agent-view/answer";
import { SKIP_WORDS, wasDismissed } from "@maxhq/shared/agent-view/dismiss";
import { isClipped, showHidden } from "@maxhq/shared/status/format";
import type {
	AttentionEvent,
	AttentionId,
	AttentionOption,
	Question,
	Timestamp,
} from "@maxhq/shared/bindings";
import type { FeedCard } from "@maxhq/shared/timeline/output";

import { type AnswerButton, answerButtons } from "./buttons";

/** A long detail shows this many lines until a tap. */
export const DETAIL_LINES = 8;

/** Whether a detail runs past the lines it shows, measured or counted by its line breaks. */
export const moreBelow = (detail: string, measuredLines: number): boolean =>
	Math.max(measuredLines, detail.split("\n").length) > DETAIL_LINES;

/** One prompt, ready to draw. */
export type Prompt = {
	readonly attention: AttentionId;
	readonly title: string;
	/** Set when maxd cut the title short, so the agent's feed says more. */
	readonly clipped: boolean;
	/** Why the agent's CLI asks, when it says. */
	readonly reason: string | null;
	/** Such as the full command. */
	readonly detail: string | null;
	/** Set when the detail is a command, shown after a prompt sign. */
	readonly command: boolean;
	/** The folder it's for, when the approval names one. */
	readonly folder: string | null;
	/** When the agent started waiting. */
	readonly since: Timestamp | null;
	readonly buttons: readonly AnswerButton[];
	/** The no that sends the reason typed beside it. */
	readonly deny: AttentionOption | null;
	/** The questions to answer, when the agent asked them one by one. */
	readonly questions: readonly Question[];
	/** Whether you can type an answer, or a reason for a no. */
	readonly acceptsText: boolean;
	readonly permission: boolean;
	/** How it was answered, or that it was dismissed, once it was. */
	readonly decided: string | null;
};

/**
 * The prompt as the inbox has it. A permission whose event carries no
 * choices is one only its card may answer, so it offers no answer here,
 * only the way to its card.
 */
export const promptOfEvent = (event: AttentionEvent): Prompt => {
	const permission = event.kind === "permission";
	const acceptsText = event.accepts_text && !(permission && event.options.length === 0);
	return {
		attention: event.id,
		title: showHidden(event.summary),
		clipped: isClipped(event.summary),
		reason: null,
		detail: null,
		command: false,
		folder: null,
		since: event.created_at,
		...answerButtons(event.options, permission, acceptsText),
		questions: [],
		acceptsText,
		permission,
		decided: null,
	};
};

/** The prompt as the feed has it, with the event while it's still open. */
export const promptOfItem = (item: FeedCard, event: AttentionEvent | undefined): Prompt => {
	const dismissed = wasDismissed(item) ? SKIP_WORDS.dismissed : null;
	if (item.kind === "approval") {
		const acceptsText = event?.accepts_text ?? false;
		return {
			attention: item.attention,
			title: showHidden(item.title),
			clipped: false,
			reason: item.reason ? showHidden(item.reason) : null,
			detail: item.detail === null ? null : showHidden(item.detail),
			command: item.approval_kind === "command",
			folder: item.cwd === null ? null : showHidden(item.cwd),
			since: item.started_at,
			...answerButtons(item.options, true, acceptsText, item.deny_first ?? false),
			questions: [],
			acceptsText,
			permission: true,
			decided: item.decided ? decidedLine(item.decided, item.options) : dismissed,
		};
	}
	const first = item.questions[0];
	return {
		attention: item.attention,
		title:
			item.questions.length === 1 && first
				? (first.consent?.title ?? first.header ?? t("Question"))
				: t("{{count}} questions", { count: item.questions.length }),
		clipped: false,
		reason: null,
		detail: null,
		command: false,
		folder: null,
		since: item.started_at,
		buttons: [],
		deny: null,
		questions: item.questions,
		acceptsText: false,
		permission: false,
		decided: item.answers
			? t("Answered: {{answers}}", {
					answers: item.answers.map((answer) => answer.values.join(", ")).join("; "),
				})
			: dismissed,
	};
};
