// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Questions the agent asks you, each with its options and a box for an
// answer of your own where it takes one. The box takes pills as the
// composer does, apart from a secret's, which stays a password field. One
// button sends every answer, and Dismiss skips them all and stops the turn.
// With the card focused, 1 to 9 pick an option of the question you're on,
// Enter sends, and Esc dismisses.

import { Ban, Check, CircleCheck, MessageCircleQuestionMark } from "lucide-react";
import { type KeyboardEvent, useCallback, useId, useState } from "react";

import type { AttentionAnswer, Question, QuestionAnswer } from "@maxhq/shared/bindings";
import { useView } from "@/app/services";
import { Tooltip } from "@/components/ui/tooltip";
import { AnswerField, useAnswerPlace } from "@/composer/answer-field";
import { cn } from "@/lib/utils";

import {
	answerRefs,
	type Choice,
	NO_CHOICE,
	plainChoices,
	questionsAnswer,
	SECRET_ASKED,
	toggled,
	useSend,
} from "@maxhq/shared/agent-view/answer";
import { useRow } from "@maxhq/shared/agent-view/context";
import {
	DISMISS,
	dismissBlocked,
	SKIP_WORDS,
	wasDismissed,
} from "@maxhq/shared/agent-view/dismiss";
import { useKept } from "@maxhq/shared/agent-view/kept";
import { type AnswerPlace, shownAnswer, takesAnswerPills } from "@maxhq/shared/pills/answer";
import type { InlineRef } from "@maxhq/shared/pills/wire";
import { EndedCard, PrimaryButton, SecondaryButton, SendNote, WaitingCard } from "./card";
import { ConsentCard } from "./consent";
import { isOpen } from "@maxhq/shared/agent-view/format";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const ICON = <MessageCircleQuestionMark size={15} strokeWidth={1.9} aria-hidden />;

const Indicator = ({ multi, on }: { multi: boolean; on: boolean }) => {
	useI18n();
	return (
		<span
			aria-hidden
			className={cn(
				"mt-px grid size-3.5 shrink-0 place-items-center border-[1.5px]",
				multi ? "rounded-[4px]" : "rounded-full",
				on
					? "border-primary bg-primary text-primary-foreground"
					: "border-subtle-foreground"
			)}
		>
			{on ? (
				multi ? (
					<Check size={10} strokeWidth={3} />
				) : (
					<span className="size-1.5 rounded-full bg-primary-foreground" />
				)
			) : null}
		</span>
	);
};

/** The answers on their way, and the pills typed in them. */
type Sent = { readonly answer: AttentionAnswer; readonly refs: readonly InlineRef[] };

/**
 * Dismiss, which skips the questions and stops the turn. While the maxd
 * on the agent's device is too old for it, it stays where it is and says
 * so on hover or focus.
 */
const DismissButton = ({
	blocked,
	disabled,
	onClick,
}: {
	/** Why it's off, or null when it's on. */
	blocked: string | null;
	disabled: boolean;
	onClick: () => void;
}) => {
	useI18n();
	const tip = useId();
	if (blocked !== null) {
		return (
			<span className="group relative inline-flex">
				<button
					type="button"
					aria-disabled="true"
					aria-describedby={tip}
					className="inline-flex h-7 cursor-not-allowed items-center rounded-md border border-input bg-secondary px-2.5 text-[12px] text-subtle-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
				>
					{SKIP_WORDS.dismiss}
				</button>
				<span
					id={tip}
					role="tooltip"
					className="pointer-events-none absolute bottom-[calc(100%+6px)] left-0 z-10 hidden w-max max-w-72 rounded-md border bg-popover px-2 py-1 text-[11.5px] leading-snug text-popover-foreground shadow-md group-focus-within:block group-hover:block"
				>
					{blocked}
				</span>
			</span>
		);
	}
	return (
		<Tooltip
			content={
				<>
					{SKIP_WORDS.dismissTip}{" "}
					<span className="text-[10.5px] text-muted-foreground">{translate("Esc")}</span>
				</>
			}
		>
			{/* The wrapper takes the tooltip, so the button keeps its own look. */}
			<span className="inline-flex">
				<SecondaryButton onClick={onClick} disabled={disabled} keys="Escape">
					{SKIP_WORDS.dismiss}
				</SecondaryButton>
			</span>
		</Tooltip>
	);
};

const OneQuestion = ({
	id,
	question,
	choice,
	onChange,
	onUse,
	onEnter,
	place,
	readOnly,
	disabled,
	keys,
}: {
	id: string;
	question: Question;
	choice: Choice;
	onChange: (choice: Choice) => void;
	/** Hears a focus or click in the question, which the number keys then pick for. */
	onUse: () => void;
	/** Enter in the box for your own answer. */
	onEnter: () => void;
	place: AnswerPlace;
	readOnly: boolean;
	disabled: boolean;
	/** Set on the question the number keys pick for, to show them. */
	keys: boolean;
}) => {
	useI18n();
	const { agent } = useRow();
	const typed = question.other || question.options.length === 0;
	const ownLabel =
		question.options.length > 0 ? translate("Your own answer") : translate("Your answer");
	const ownHint =
		question.options.length > 0 ? translate("Something else") : translate("Type your answer");
	return (
		// WebKit doesn't focus a clicked button, so a click counts too.
		<div className="flex min-w-0 flex-col gap-1.5" onFocus={onUse} onPointerDown={onUse}>
			{question.header ? (
				<span className="text-[11.5px] font-medium text-status-ready-text">
					{question.header}
				</span>
			) : null}
			<p
				id={`${id}-text`}
				className="max-h-[60vh] overflow-y-auto text-[13px] leading-normal break-words whitespace-pre-wrap text-foreground-strong select-text"
			>
				{question.text}
			</p>
			{question.options.length > 0 ? (
				<div
					role={question.multi ? "group" : "radiogroup"}
					aria-labelledby={`${id}-text`}
					className="flex flex-col gap-1"
				>
					{question.options.map((option, index) => {
						const on = choice.values.includes(option.label);
						return (
							<button
								key={option.label}
								type="button"
								role={question.multi ? "checkbox" : "radio"}
								aria-checked={on}
								disabled={readOnly || disabled}
								onClick={() => onChange(toggled(question, choice, option.label))}
								className={cn(
									"flex w-full items-start gap-2.5 rounded-md border px-2.5 py-1.5 text-left",
									on
										? "border-selected-ring bg-selected"
										: "border-input bg-background/40 enabled:hover:bg-accent",
									!readOnly && "enabled:cursor-pointer"
								)}
							>
								<Indicator multi={question.multi} on={on} />
								<span className="flex min-w-0 flex-1 flex-col gap-px">
									<span className="text-[12.5px] text-foreground">
										{option.label}
									</span>
									{option.description ? (
										<span className="text-[11.5px] text-muted-foreground">
											{option.description}
										</span>
									) : null}
								</span>
								{keys && index < 9 ? (
									<kbd
										aria-hidden
										className="hidden shrink-0 font-sans text-[10.5px] leading-[18px] text-muted-foreground tabular-nums group-focus-within/card:inline"
									>
										{index + 1}
									</kbd>
								) : null}
							</button>
						);
					})}
				</div>
			) : null}
			{typed && !readOnly && question.secret ? (
				<>
					<label className="sr-only" htmlFor={`${id}-other`}>
						{ownLabel}
					</label>
					<input
						id={`${id}-other`}
						type="password"
						autoComplete="off"
						disabled={disabled}
						value={choice.other}
						onChange={(event) =>
							onChange({
								values: question.multi ? choice.values : [],
								other: event.target.value,
							})
						}
						placeholder={ownHint}
						className="h-7 rounded-md border border-input bg-background px-[9px] text-[12px] text-foreground outline-none select-text placeholder:text-subtle-foreground focus:border-ring"
					/>
					<p className="text-[11.5px] text-muted-foreground">{SECRET_ASKED}</p>
				</>
			) : typed && !readOnly ? (
				<AnswerField
					value={{ text: choice.other, refs: choice.refs ?? [] }}
					onChange={(own) =>
						onChange({
							values: question.multi ? choice.values : [],
							other: own.text,
							refs: own.refs,
						})
					}
					onEnter={onEnter}
					label={ownLabel}
					placeholder={ownHint}
					agent={agent}
					place={place}
					device={place.device}
					look="card"
					busy={disabled}
				/>
			) : null}
		</div>
	);
};

const LiveQuestion = ({ item }: { item: ItemOf<"question"> }) => {
	useI18n();
	const { agent, vendor, readOnly, answer } = useRow();
	const [choices, setChoices] = useKept<Readonly<Record<string, Choice>>>(
		`choices:${item.attention}`,
		{}
	);
	const place = useAnswerPlace(agent);
	// A maxd too old for pills gets each one as its words.
	const pills = useView(
		(state) => place.device !== null && takesAnswerPills(state.view, place.device)
	);
	const send = useCallback(
		({ answer: a, refs }: Sent) => answer(item.attention, a, refs),
		[answer, item.attention]
	);
	const [state, submit] = useSend(send);
	const answered = questionsAnswer(item.questions, pills ? choices : plainChoices(choices));
	const ready: Sent | null = answered && {
		answer: answered,
		refs: pills ? answerRefs(item.questions, choices) : [],
	};
	const busy = state.state === "sending";
	const blocked = useView((s) => dismissBlocked(s.view, place.device));
	// Whether the last thing sent was Dismiss, so the answer button doesn't spin for it.
	const [skipping, setSkipping] = useState(false);
	const many = item.questions.length > 1;
	// The question the number keys pick for: the first, or the one you're on.
	const [current, setCurrent] = useState(0);

	const sendAnswers = () => {
		if (!ready || busy || readOnly) return;
		setSkipping(false);
		submit(ready);
	};

	const dismiss = () => {
		if (busy || readOnly || blocked !== null) return;
		setSkipping(true);
		submit({ answer: DISMISS, refs: [] });
	};

	const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
		if (readOnly || busy || event.metaKey || event.ctrlKey || event.altKey) return;
		// Esc anywhere in the card, unless the box for your answer used it first.
		if (event.key === "Escape") {
			if (event.defaultPrevented || event.nativeEvent.isComposing || blocked !== null) return;
			event.preventDefault();
			event.stopPropagation();
			dismiss();
			return;
		}
		const at = event.target;
		if (
			at instanceof HTMLElement &&
			(at.isContentEditable || at.matches("input, textarea, [contenteditable='true']"))
		) {
			return;
		}
		if (/^[1-9]$/.test(event.key)) {
			const question = item.questions[current];
			const option = question?.options[Number(event.key) - 1];
			if (!question || !option) return;
			event.preventDefault();
			setChoices({
				...choices,
				[question.id]: toggled(question, choices[question.id] ?? NO_CHOICE, option.label),
			});
			// One pick answers a single-choice question, so the keys move on to the next.
			if (!question.multi && current < item.questions.length - 1) setCurrent(current + 1);
			return;
		}
		// Enter on the card itself sends, as the hint on its button says.
		if (event.key === "Enter" && at === event.currentTarget && ready) {
			event.preventDefault();
			sendAnswers();
		}
	};

	return (
		<WaitingCard
			label={many ? translate("Questions") : translate("Question")}
			icon={ICON}
			title={
				many
					? translate("{{length}} questions for you", { length: item.questions.length })
					: translate("A question for you")
			}
			vendor={vendor}
			since={item.started_at}
			onKeyDown={onKeyDown}
		>
			<form
				className="flex flex-col gap-3"
				onSubmit={(event) => {
					event.preventDefault();
					sendAnswers();
				}}
			>
				{item.questions.map((question, index) => (
					<OneQuestion
						key={question.id}
						id={`${item.id}-${question.id}`}
						question={question}
						choice={choices[question.id] ?? NO_CHOICE}
						onChange={(choice) => setChoices({ ...choices, [question.id]: choice })}
						onUse={() => setCurrent(index)}
						onEnter={sendAnswers}
						place={place}
						readOnly={readOnly}
						disabled={busy}
						keys={!readOnly && index === current}
					/>
				))}
				{readOnly ? (
					<p className="text-[11.5px] text-muted-foreground">
						{translate(
							"Answer it in the terminal. MaxHQ can't answer a session it doesn't hold."
						)}
					</p>
				) : (
					<div className="flex items-center gap-2">
						<PrimaryButton submit busy={busy && !skipping} disabled={!ready || busy}>
							{many ? translate("Send answers") : translate("Send answer")}
						</PrimaryButton>
						<DismissButton blocked={blocked} disabled={busy} onClick={dismiss} />
						{!ready ? (
							<span className="text-[11.5px] text-muted-foreground">
								{many
									? translate("Answer each question to send.")
									: translate("Pick an answer to send.")}
							</span>
						) : null}
					</div>
				)}
				<SendNote state={state} vendor={vendor} dismissed={skipping} />
			</form>
		</WaitingCard>
	);
};

const answerText = (question: Question | undefined, answer: QuestionAnswer): string =>
	question?.secret ? translate("Hidden") : answer.values.map(shownAnswer).join(", ");

/** Questions, waiting or answered. */
export const QuestionCard = ({ item }: { item: ItemOf<"question"> }) => {
	useI18n();
	if (item.answers) {
		const byId = new Map(item.questions.map((q) => [q.id, q]));
		return (
			<EndedCard
				label={translate("Answered question")}
				icon={
					<CircleCheck
						size={14}
						strokeWidth={1.9}
						aria-hidden
						className="text-status-ready"
					/>
				}
				title={
					item.questions.length > 1
						? translate("Answered the questions")
						: translate("Answered the question")
				}
			>
				<dl className="ml-[22px] flex flex-col gap-1 text-[12px]">
					{item.answers.map((answer) => {
						const question = byId.get(answer.id);
						return (
							<div key={answer.id} className="flex min-w-0 flex-col">
								<dt className="text-muted-foreground">
									{question?.header ?? question?.text ?? answer.id}
								</dt>
								<dd className="break-words text-secondary-foreground select-text">
									{answerText(question, answer)}
								</dd>
							</div>
						);
					})}
				</dl>
			</EndedCard>
		);
	}
	const [first] = item.questions;
	if (isOpen(item.status)) {
		return first?.consent ? (
			<ConsentCard item={item} question={first} consent={first.consent} />
		) : (
			<LiveQuestion item={item} />
		);
	}
	const dismissed = wasDismissed(item);
	return (
		<EndedCard
			label={dismissed ? SKIP_WORDS.dismissed : translate("Question closed")}
			icon={
				<Ban size={14} strokeWidth={1.9} aria-hidden className="text-subtle-foreground" />
			}
			title={first?.consent?.title ?? first?.text ?? translate("A question")}
			meta={dismissed ? SKIP_WORDS.dismissed : "Closed without an answer"}
		/>
	);
};
