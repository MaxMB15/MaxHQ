// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { StyleSheet, Text, TextInput, View } from "react-native";
import { useStore } from "zustand";

import { keptValue, useKept } from "@maxhq/shared/agent-view/kept";
import type { Agent, SendMode, TimelineItem } from "@maxhq/shared/bindings";
import {
	attempt,
	type ComposerAction,
	failureOf,
	placeholderFor,
	type QueuedMessage,
	waitingMessages,
} from "@maxhq/shared/composer/send";
import { draftText, textDraft } from "@maxhq/shared/composer/saved-drafts";
import type { ActionResult } from "@maxhq/shared/data/source";
import { agreeToSend } from "@maxhq/shared/settings/auto-mode";

import { useServices } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { askAutoDefault, autoConsent } from "./auto-consent";
import {
	canSend,
	draftKey,
	inputOf,
	otherSend,
	sendAgreed,
	sendLabel,
	sendMode,
	stopsTurn,
} from "./compose";
import { QueuedList } from "./Queued";
import { savedDrafts } from "./saved-drafts";
import { busySendPref } from "./send-pref";

/**
 * The box under an agent's output, bordered as the desktop's, with the send
 * button inside it and the messages that wait for the turn above it. Once
 * you type, a second button beside it steers or queues, whichever the send
 * button doesn't, while a turn runs, and skips the prompt while one waits.
 * What you type stays with the agent until it goes, and the phone saves it,
 * so it comes back after a restart.
 */
export const Composer = ({
	agent,
	device,
	queued,
}: {
	agent: Agent;
	device: string;
	/** The agent's messages that wait for the turn to end. */
	queued: readonly TimelineItem[];
}) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	// What you typed while the app ran, or else what the phone saved before.
	const [saved] = useState(
		() =>
			keptValue<string | null>(draftKey(agent.id), null) ??
			draftText(savedDrafts.get(agent.id))
	);
	const [text, keepText] = useKept(draftKey(agent.id), saved);
	const setText = (next: string) => {
		keepText(next);
		savedDrafts.save(agent.id, textDraft(next));
	};
	const [sending, setSending] = useState(false);
	const [error, setError] = useState<string | null>(null);
	// Messages taken back, until the timeline drops them, and those on their way back.
	const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
	const [pending, setPending] = useState<ReadonlySet<string>>(() => new Set());
	const waiting = useMemo(() => waitingMessages(queued, hidden), [queued, hidden]);
	const input = inputOf(text);
	const open = canSend(agent.activity);
	const pick = useStore(busySendPref, (s) => s.pick);
	const stop = stopsTurn(agent.activity, text);
	const other = otherSend(agent.activity, pick, text);
	const { label, hint } = stop
		? { label: t("Stop"), hint: null }
		: sendLabel(agent.activity, pick);

	/** Runs an action and shows why it failed, if it did. Returns whether it went through. */
	const run = async (
		what: ComposerAction,
		action: () => Promise<ActionResult>
	): Promise<boolean> => {
		setError(null);
		const failure = failureOf(await attempt(action), what);
		setError(failure);
		return failure === null;
	};

	const press = async (mode: SendMode) => {
		setSending(true);
		if (stop) await run("stop", () => source.interrupt(agent.id));
		else if (input) {
			setError(null);
			// An Auto you didn't pick, as the device's default, asks first.
			const { sent, failure } = await sendAgreed(
				() =>
					agreeToSend(
						agent,
						autoConsent,
						askAutoDefault,
						source,
						services.view.getState().view
					),
				() => source.sendInput(agent.id, input, mode)
			);
			if (sent) setText("");
			setError(failure);
		}
		setSending(false);
	};

	const mark = (id: string, on: boolean, set: typeof setPending) =>
		set((current) => {
			const next = new Set(current);
			if (on) next.add(id);
			else next.delete(id);
			return next;
		});

	const remove = async (message: QueuedMessage) => {
		mark(message.id, true, setPending);
		if (await run("unqueue", () => source.cancelQueued(agent.id, message.id)))
			mark(message.id, true, setHidden);
		mark(message.id, false, setPending);
	};

	return (
		<View style={styles.bar}>
			<QueuedList messages={waiting} pending={pending} onRemove={(m) => void remove(m)} />
			{error ? <Text style={styles.failed}>{error}</Text> : null}
			<View style={styles.box}>
				<TextInput
					style={styles.input}
					value={text}
					onChangeText={setText}
					editable={open && !sending}
					multiline
					placeholder={placeholderFor(agent.activity, agent.title, device)}
					placeholderTextColor={color.subtleForeground}
					accessibilityLabel={t("Message")}
				/>
				<View style={styles.row}>
					<Text style={styles.hint} numberOfLines={1}>
						{hint ?? ""}
					</Text>
					{other ? (
						<Button
							label={other.label}
							hint={other.hint}
							small
							disabled={!open || sending}
							onPress={() => void press(other.mode)}
						/>
					) : null}
					<Button
						label={label}
						tone={stop ? "secondary" : "primary"}
						small
						disabled={!open || sending || (!stop && !input)}
						onPress={() => void press(sendMode(agent.activity, pick))}
					/>
				</View>
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	bar: { backgroundColor: color.background, paddingHorizontal: 10, paddingVertical: 8, gap: 6 },
	box: {
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
		borderRadius: 10,
		backgroundColor: color.subpanel,
		paddingHorizontal: 12,
		paddingTop: 10,
		paddingBottom: 8,
		gap: 6,
	},
	input: {
		minHeight: 36,
		maxHeight: 120,
		padding: 0,
		color: color.foreground,
		fontSize: size.body,
		lineHeight: 20,
	},
	row: { flexDirection: "row", alignItems: "center", gap: 8 },
	hint: { flex: 1, color: color.subtleForeground, fontSize: size.tiny },
	failed: { color: color.errorText, fontSize: size.meta },
});
