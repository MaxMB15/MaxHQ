// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the approval and question cards turn what you click and type into an
// `AttentionAnswer`, and how they describe an answer once it's in.

import { t } from "../i18n";

import { useCallback, useState } from "react";

import type { ActionResult } from "../data/source";
import type {
	AttentionAnswer,
	AttentionOption,
	ClientId,
	Decided,
	Question,
	QuestionAnswer,
} from "../bindings";
import { plainOf } from "../pills/answer";
import { gitKey, isPlaced } from "../pills/draft";
import type { InlineRef } from "../pills/wire";
import { formatClock, sentence, showHidden } from "../status/format";
import { errorText } from "../timeline/store";

const DENY =
	/^(deny|decline|reject|abort|cancel|no(?![-\w])|not[ _]now|block|skip|never(?![ _]ask)|stop(?![ _]ask)|don['’]?t[ _](allow|run)|do[ _]not[ _](allow|run)|always[ _](block|deny))/i;

// What the agent says an option does. Its label can say anything, so this
// decides first, and the id and label only where the agent says nothing.
const said = (option: AttentionOption) => /^(allow|reject)_(once|always)$/.exec(option.kind ?? "");

/** Whether an option turns the request down. */
export const isDeny = (option: AttentionOption): boolean => {
	const kind = said(option);
	return kind ? kind[1] === "reject" : DENY.test(option.id) || DENY.test(option.label);
};

const LASTING = /always|session|forever|permanent|again|no[-_ ]?(ask|prompt)/i;

// maxd's own options that outlast the request: accepting every edit after a
// plan, and a grant for the rest of Codex's turn.
const LASTING_ID = /^(accept_edits|grant_turn)$/;

// Whether an option goes beyond this one request.
const lasts = (option: AttentionOption): boolean => {
	const kind = said(option);
	return kind
		? kind[2] === "always"
		: LASTING_ID.test(option.id) || LASTING.test(option.id) || LASTING.test(option.label);
};

/** An approval's options as the card lays them out. */
export type Choices = {
	/** The first yes for this request alone, on the main button. A yes that lasts never goes there. */
	readonly primary: AttentionOption | null;
	/** Every other option, in the order the agent listed them. */
	readonly others: readonly AttentionOption[];
	/** The option that says no, beside the box for a reason. */
	readonly deny: AttentionOption | null;
};

/**
 * Sorts an approval's options into the card's buttons. The agent names and
 * orders them, so the main button goes by what an option says, not by where
 * it's listed.
 */
export const approvalChoices = (options: readonly AttentionOption[]): Choices => {
	const shown = options.map((o) => ({ ...o, label: showHidden(o.label) }));
	const deny = shown.find((o) => isDeny(o) && !lasts(o)) ?? shown.find(isDeny) ?? null;
	const yes = shown.filter((o) => !isDeny(o));
	const primary = yes.find((o) => !lasts(o)) ?? null;
	return { primary, others: shown.filter((o) => o !== deny && o !== primary), deny };
};

/** The answer that turns a request down, with your reason when you gave one. */
export const denyAnswer = (deny: AttentionOption, reason: string): AttentionAnswer => {
	const text = reason.trim();
	return text ? { type: "text", text } : { type: "option", id: deny.id };
};

/** What you picked and typed for one question. */
export type Choice = {
	readonly values: readonly string[];
	/**
	 * What you typed as your own answer, when the question allows one, with
	 * a mark where each placed pill sits.
	 */
	readonly other: string;
	/** The pills in what you typed. */
	readonly refs?: readonly InlineRef[];
};

/** No answer yet. */
export const NO_CHOICE: Choice = { values: [], other: "" };

/** One option chosen, or unchosen for a question that takes several. */
export const toggled = (question: Question, choice: Choice, label: string): Choice => {
	if (!question.multi) return { values: [label], other: "" };
	const values = choice.values.includes(label)
		? choice.values.filter((v) => v !== label)
		: [...choice.values, label];
	return { ...choice, values };
};

/** The values a choice sends, in the order the question lists its options. */
export const choiceValues = (question: Question, choice: Choice): string[] => {
	const other = choice.other.trim();
	if (!question.multi && other) return [other];
	const listed = question.options.map((o) => o.label).filter((l) => choice.values.includes(l));
	return other ? [...listed, other] : listed;
};

/** The answer to a card's questions, or null while one has nothing picked. */
export const questionsAnswer = (
	questions: readonly Question[],
	choices: Readonly<Record<string, Choice>>
): AttentionAnswer | null => {
	const answers: QuestionAnswer[] = [];
	for (const question of questions) {
		const values = choiceValues(question, choices[question.id] ?? NO_CHOICE);
		if (values.length === 0) return null;
		answers.push({ id: question.id, values });
	}
	return { type: "questions", answers };
};

/**
 * The pills in a card's answers, in the order their marks come: question
 * by question, as each one's own answer comes last among its values. Each
 * pull request or issue goes once.
 */
export const answerRefs = (
	questions: readonly Question[],
	choices: Readonly<Record<string, Choice>>
): InlineRef[] => {
	const placed: InlineRef[] = [];
	const items = new Map<string, InlineRef>();
	for (const question of questions) {
		const choice = choices[question.id] ?? NO_CHOICE;
		if (!choice.other.trim()) continue;
		for (const ref of choice.refs ?? []) {
			if (isPlaced(ref)) placed.push(ref);
			else items.set(gitKey(ref), ref);
		}
	}
	return [...placed, ...items.values()];
};

/** Each answer you typed with its pills as plain words, for a maxd too old for them. */
export const plainChoices = (choices: Readonly<Record<string, Choice>>): Record<string, Choice> =>
	Object.fromEntries(
		Object.entries(choices).map(([id, choice]) => [
			id,
			choice.refs?.length
				? {
						values: choice.values,
						other: plainOf({ text: choice.other, refs: choice.refs }),
					}
				: choice,
		])
	);

/** Where an answer came from, such as "in the terminal" or "from MaxHQ". */
export const answeredFrom = (by: ClientId | null): string =>
	by === null ? t("in the terminal") : t("from MaxHQ");

/** The line a decided approval shows, such as "Allow once, from MaxHQ at 14:21". */
export const decidedLine = (decided: Decided, options: readonly AttentionOption[]): string => {
	const where = t("{{value1}} at {{value2}}", {
		value1: answeredFrom(decided.by),
		value2: formatClock(decided.at),
	});
	const { answer } = decided;
	if (answer.type === "option") {
		const label = options.find((o) => o.id === answer.id)?.label ?? answer.id;
		return `${label}, ${where}`;
	}
	if (answer.type === "text") return t("Answered with a note, {{where}}", { where: where });
	return t("Answered, {{where}}", { where: where });
};

/** Whether a decided approval said no. */
export const decidedNo = (decided: Decided, options: readonly AttentionOption[]): boolean => {
	const { answer } = decided;
	if (answer.type === "text") return true;
	if (answer.type !== "option") return false;
	const option = options.find((o) => o.id === answer.id);
	return option ? isDeny(option) : false;
};

/** Where an answer is on its way. */
export type SendState =
	| { readonly state: "idle" }
	| { readonly state: "sending" }
	| { readonly state: "sent" }
	/** Another answer got there first. */
	| { readonly state: "late" }
	| { readonly state: "failed"; readonly message: string };

/** Beside a password field, so an agent's request doesn't pass for the app's own. */
export const SECRET_ASKED = "The agent is asking for this, not MaxHQ.";

/** What a card says once another answer got there first. */
export const ALREADY_ANSWERED = "Already answered";

// The refusals the core sends when the request was answered before yours
// arrived: from another client or the terminal, or so long ago the device
// has let it go.
const LATE = new Set(["Someone else answered first", "The device no longer has this request"]);

/** Whether an answer was turned away because another one got there first. */
export const isLate = (result: ActionResult): boolean =>
	result.status === "ok" && result.data.status === "refused" && LATE.has(result.data.reason);

/** Why an answer didn't go through, or null when it did. */
export const failureOf = (result: ActionResult): string | null => {
	if (result.status === "error") return errorText(result.error);
	switch (result.data.status) {
		case "refused":
			return `${sentence(result.data.reason)}.`;
		case "unknown_agent":
			return t("MaxHQ no longer knows this agent.");
		default:
			return null;
	}
};

/** Sends answers one at a time and tracks how the last one went. */
export const useSend = <A = AttentionAnswer>(
	send: (answer: A) => Promise<ActionResult>
): [SendState, (answer: A) => void] => {
	const [state, setState] = useState<SendState>({ state: "idle" });
	const submit = useCallback(
		(answer: A) => {
			setState({ state: "sending" });
			send(answer).then(
				(result) => {
					if (isLate(result)) return setState({ state: "late" });
					const failure = failureOf(result);
					setState(failure ? { state: "failed", message: failure } : { state: "sent" });
				},
				(error: unknown) =>
					setState({
						state: "failed",
						message:
							error instanceof Error
								? error.message
								: t("The answer didn't go through."),
					})
			);
		},
		[send]
	);
	return [state, submit];
};
