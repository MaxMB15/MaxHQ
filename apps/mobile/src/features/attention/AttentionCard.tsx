// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A prompt an agent is waiting on, laid out as the desktop's waiting card:
// what it asks and why, when the agent says, the full command in a box,
// which a tap opens in full, the edit it asks about, the yes buttons, and a
// box for a reason with the no on its end. When the agent wants no as the
// default, the no leads the buttons instead. A question shows each of its
// questions with their options and a box for your own answer, and Dismiss
// beside its submit button skips them and stops the turn. Once it's
// answered it says how, and an answer that came too late says so.

import { useI18n } from "@maxhq/shared/i18n";

import * as Haptics from "expo-haptics";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import {
	ALREADY_ANSWERED,
	type Choice,
	denyAnswer,
	NO_CHOICE,
	questionsAnswer,
	SECRET_ASKED,
	toggled,
	useSend,
} from "@maxhq/shared/agent-view/answer";
import { DISMISS, dismissSent, SKIP_WORDS } from "@maxhq/shared/agent-view/dismiss";
import { baseName } from "@maxhq/shared/agent-view/format";
import type { AttentionAnswer, FileEdit, Question, Vendor } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatAgoShort } from "@maxhq/shared/status/format";
import { VENDOR_NAME } from "@maxhq/shared/status/usage";

import { useServices } from "../../services";
import { color, font, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { ActivityDot } from "../../ui/Dot";
import { AttachedField, Input } from "../../ui/Field";
import { PatchView } from "../agent/PatchView";
import { ConsentBlock } from "./ConsentBlock";
import { DETAIL_LINES, moreBelow, type Prompt } from "./prompt";

// How much of a row each button takes, as the desktop's: the yes, the
// others a little wider for their longer words, and a no a little narrower.
const GROW = { primary: 1, secondary: 1.5, danger: 0.9 } as const;

/** Where a question card offers Dismiss, and why it's off, when it is. */
export type Dismissal = { readonly blocked: string | null };

export const AttentionCard = ({
	prompt,
	vendor,
	title,
	place,
	tag,
	onOpen,
	changes,
	dismiss,
}: {
	prompt: Prompt;
	vendor: Vendor;
	/** The agent's title, set in the inbox, where the card sits among others. */
	title?: string;
	/** The device the agent runs on. */
	place?: string;
	/** The tag naming the agent's Mac, while several Macs show together. */
	tag?: ReactNode;
	onOpen?: () => void;
	/** The edit it asks about, when it asks to change files. */
	changes?: readonly FileEdit[] | null;
	/** Set where its questions can be dismissed: in the feed of an agent MaxHQ runs. */
	dismiss?: Dismissal;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(60_000);
	const send = useCallback(
		(answer: AttentionAnswer) => source.answer(prompt.attention, answer),
		[source, prompt.attention]
	);
	const [state, submit] = useSend(send);
	// Whether the last thing sent was Dismiss, which stops the turn rather than answer.
	const [skipping, setSkipping] = useState(false);
	// You feel whether an answer went, without looking.
	useEffect(() => {
		const { Success, Warning } = Haptics.NotificationFeedbackType;
		if (state.state === "sent") void Haptics.notificationAsync(Success);
		if (state.state === "failed") void Haptics.notificationAsync(Warning);
	}, [state.state]);
	const [text, setText] = useState("");
	const [choices, setChoices] = useState<Record<string, Choice>>({});
	const [full, setFull] = useState(false);
	const [lines, setLines] = useState(0);
	const busy = state.state === "sending" || state.state === "sent";
	const open = prompt.decided === null && state.state !== "late" && state.state !== "sent";
	const questions =
		prompt.questions.length > 0 ? questionsAnswer(prompt.questions, choices) : null;
	const choose = (id: string, choice: Choice) => setChoices((all) => ({ ...all, [id]: choice }));
	// A card that asks for your consent lays itself out, with its own buttons.
	const [first] = prompt.questions;
	const consent = first?.consent ?? null;
	const inbox = title !== undefined;
	const ago = prompt.since !== null ? formatAgoShort(prompt.since, now) : null;
	const waiting =
		ago === null || ago === t("now")
			? t("{{vendor}} is waiting", { vendor: VENDOR_NAME[vendor] })
			: t("{{vendor}} is waiting, {{time}}", { vendor: VENDOR_NAME[vendor], time: ago });
	const where =
		prompt.folder !== null
			? place
				? t("In {{folder}} on {{device}}.", {
						folder: baseName(prompt.folder),
						device: place,
					})
				: t("In {{folder}}.", { folder: baseName(prompt.folder) })
			: inbox && place
				? t("On {{device}}.", { device: place })
				: null;

	return (
		<View
			style={[styles.card, inbox ? styles.inboxCard : styles.feedCard]}
			accessibilityLabel={prompt.permission ? t("Permission request") : t("Question")}
		>
			<Pressable
				disabled={!onOpen}
				onPress={onOpen}
				accessibilityRole={onOpen ? "link" : undefined}
				style={styles.top}
			>
				<View style={styles.head}>
					{inbox ? (
						<ActivityDot look="needs_input" surface={color.card} decorative />
					) : null}
					<Text style={styles.title} numberOfLines={inbox ? 1 : 2}>
						{title ?? prompt.title}
					</Text>
					{tag}
					{open ? <Text style={styles.waiting}>{waiting}</Text> : null}
				</View>
				{prompt.reason ? <Why text={prompt.reason} /> : null}
				{inbox && prompt.detail === null ? (
					<Text style={styles.summary}>{prompt.title}</Text>
				) : null}
				{inbox && prompt.detail === null && prompt.clipped && onOpen ? (
					<Text style={styles.where}>{t("Open for the full request.")}</Text>
				) : null}
				{prompt.detail ? (
					<Pressable
						accessibilityRole="button"
						accessibilityState={{ expanded: full }}
						onPress={() => setFull((f) => !f)}
					>
						<Text style={styles.detail} numberOfLines={full ? undefined : DETAIL_LINES}>
							{prompt.command ? <Text style={styles.sign}>$ </Text> : null}
							{prompt.detail}
						</Text>
						{/* Laid out in full behind the box to count its lines. */}
						<Text
							style={[styles.detail, styles.measure]}
							accessibilityElementsHidden
							importantForAccessibility="no-hide-descendants"
							onTextLayout={(event) => setLines(event.nativeEvent.lines.length)}
						>
							{prompt.command ? "$ " : ""}
							{prompt.detail}
						</Text>
						{!full && moreBelow(prompt.detail, lines) ? (
							<Text style={styles.where}>{t("More below. Tap to read it all.")}</Text>
						) : null}
					</Pressable>
				) : null}
				{where ? <Text style={styles.where}>{where}</Text> : null}
			</Pressable>
			{changes ? <PatchView changes={changes} /> : null}
			{open && first && consent ? (
				<ConsentBlock question={first} consent={consent} busy={busy} onSubmit={submit} />
			) : open ? (
				prompt.questions.map((question) => (
					<QuestionBlock
						key={question.id}
						question={question}
						choice={choices[question.id] ?? NO_CHOICE}
						onChange={(choice) => choose(question.id, choice)}
					/>
				))
			) : null}
			{open && prompt.buttons.length > 0 ? (
				<View style={styles.buttons}>
					{prompt.buttons.map(({ option, tone }) => (
						<Button
							key={option.id}
							label={option.label}
							tone={tone}
							grow={GROW[tone]}
							disabled={busy}
							onPress={() => submit({ type: "option", id: option.id })}
						/>
					))}
				</View>
			) : null}
			{open && prompt.deny ? (
				<AttachedField
					value={text}
					onChange={setText}
					placeholder={t("Why not, or what to do instead")}
					label={vendor === "codex" ? t("Reason for declining") : t("Reason for denying")}
					button={prompt.deny.label}
					danger
					disabled={busy}
					onSubmit={() => prompt.deny && submit(denyAnswer(prompt.deny, text))}
				/>
			) : open && prompt.acceptsText ? (
				<AttachedField
					value={text}
					onChange={setText}
					placeholder={
						prompt.permission ? t("Or say what to do instead") : t("Your answer")
					}
					label={t("Your answer")}
					button={t("Send")}
					disabled={busy}
					needsText
					onSubmit={() => submit({ type: "text", text: text.trim() })}
				/>
			) : null}
			{open && prompt.questions.length > 0 && !consent ? (
				<View style={styles.buttons}>
					<Button
						label={t("Submit answers")}
						tone="primary"
						grow={GROW.primary}
						disabled={busy || questions === null}
						onPress={() => {
							if (!questions) return;
							setSkipping(false);
							submit(questions);
						}}
					/>
					{dismiss ? (
						<Button
							label={SKIP_WORDS.dismiss}
							hint={dismiss.blocked ?? SKIP_WORDS.dismissTip}
							disabled={busy || dismiss.blocked !== null}
							onPress={() => {
								setSkipping(true);
								submit(DISMISS);
							}}
						/>
					) : null}
				</View>
			) : null}
			{open && prompt.questions.length > 0 && !consent && dismiss?.blocked ? (
				<Text style={styles.where}>{dismiss.blocked}</Text>
			) : null}
			{open &&
			prompt.buttons.length === 0 &&
			prompt.questions.length === 0 &&
			!prompt.acceptsText &&
			onOpen ? (
				<Text style={styles.note}>{t("Open the agent to answer.")}</Text>
			) : null}
			<Outcome
				decided={prompt.decided}
				state={state.state}
				sent={skipping ? dismissSent(vendor) : null}
			/>
			{state.state === "failed" ? <Text style={styles.failed}>{state.message}</Text> : null}
		</View>
	);
};

/** A reason shows this many lines until a tap. */
const WHY_LINES = 3;

/** Why the agent's CLI asks, a few lines until a tap opens it in full. */
const Why = ({ text }: { text: string }) => {
	const { t } = useI18n();
	const [full, setFull] = useState(false);
	const [lines, setLines] = useState(0);
	const line = t("Why it asks: {{reason}}", { reason: text });
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityState={{ expanded: full }}
			onPress={() => setFull((f) => !f)}
		>
			<Text style={styles.why} numberOfLines={full ? undefined : WHY_LINES}>
				{line}
			</Text>
			{/* Laid out in full behind the line to count its lines. */}
			<Text
				style={[styles.why, styles.measure]}
				accessibilityElementsHidden
				importantForAccessibility="no-hide-descendants"
				onTextLayout={(event) => setLines(event.nativeEvent.lines.length)}
			>
				{line}
			</Text>
			{!full && lines > WHY_LINES ? (
				<Text style={styles.where}>{t("More below. Tap to read it all.")}</Text>
			) : null}
		</Pressable>
	);
};

const Outcome = ({
	decided,
	state,
	sent,
}: {
	decided: string | null;
	state: string;
	/** What to say once it went, when that isn't just "Sent". */
	sent: string | null;
}) => {
	const { t } = useI18n();

	const line =
		state === "late"
			? `${t(ALREADY_ANSWERED)}.`
			: decided
				? decided
				: state === "sent"
					? (sent ?? t("Sent"))
					: state === "sending"
						? t("Sending")
						: null;
	if (!line) return null;
	return (
		<Text accessibilityRole="text" accessibilityLiveRegion="polite" style={styles.note}>
			{line}
		</Text>
	);
};

const QuestionBlock = ({
	question,
	choice,
	onChange,
}: {
	question: Question;
	choice: Choice;
	onChange: (choice: Choice) => void;
}) => {
	const { t } = useI18n();
	return (
		<View style={styles.question}>
			{question.header ? <Text style={styles.header}>{question.header}</Text> : null}
			<Text style={styles.summary}>{question.text}</Text>
			{question.options.map((option) => {
				const picked = choice.values.includes(option.label);
				return (
					<Pressable
						key={option.label}
						accessibilityRole={question.multi ? "checkbox" : "radio"}
						accessibilityState={{ checked: picked }}
						onPress={() => onChange(toggled(question, choice, option.label))}
						style={[styles.option, picked && styles.picked]}
					>
						<Text style={styles.optionLabel}>{option.label}</Text>
						{option.description ? (
							<Text style={styles.where}>{option.description}</Text>
						) : null}
					</Pressable>
				);
			})}
			{question.other ? (
				<Input
					value={choice.other}
					onChangeText={(other) =>
						onChange({ values: question.multi ? choice.values : [], other })
					}
					placeholder={t("Your own answer")}
					accessibilityLabel={t("Your own answer")}
					secureTextEntry={question.secret}
					autoCapitalize={question.secret ? "none" : "sentences"}
					style={styles.other}
				/>
			) : null}
			{question.other && question.secret ? (
				<Text style={styles.where}>{t(SECRET_ASKED)}</Text>
			) : null}
		</View>
	);
};

const styles = StyleSheet.create({
	card: { borderWidth: StyleSheet.hairlineWidth, gap: 10 },
	inboxCard: {
		borderRadius: radius.md,
		borderColor: tint(color.ready, 0.4),
		backgroundColor: color.card,
		padding: 10,
	},
	feedCard: {
		borderRadius: 10,
		borderColor: tint(color.ready, 0.45),
		backgroundColor: tint(color.ready, 0.07),
		padding: 12,
	},
	top: { gap: 6 },
	head: { flexDirection: "row", alignItems: "center", gap: 9 },
	title: {
		flex: 1,
		minWidth: 0,
		color: color.foregroundStrong,
		fontSize: size.body,
		fontWeight: "600",
	},
	waiting: { color: color.readyText, fontSize: size.tiny },
	summary: { color: color.foreground, fontSize: size.small },
	why: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	detail: {
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.meta,
		lineHeight: 18,
		paddingHorizontal: 10,
		paddingVertical: 8,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.background,
		overflow: "hidden",
	},
	measure: { position: "absolute", left: 0, right: 0, opacity: 0 },
	sign: { color: color.mutedForeground },
	where: { color: color.mutedForeground, fontSize: size.tiny },
	buttons: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
	question: { gap: 8 },
	header: { color: color.mutedForeground, fontSize: size.meta, fontWeight: "600" },
	option: {
		minHeight: 44,
		justifyContent: "center",
		paddingHorizontal: 10,
		paddingVertical: 8,
		borderRadius: radius.sm,
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
		backgroundColor: color.secondary,
	},
	picked: { borderColor: tint(color.primary, 0.6), backgroundColor: tint(color.primary, 0.14) },
	optionLabel: { color: color.foreground, fontSize: size.small },
	other: { fontSize: size.small, backgroundColor: color.background },
	note: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText, fontSize: size.meta },
});
