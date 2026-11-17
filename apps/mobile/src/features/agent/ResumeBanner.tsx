// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Over the composer once an agent MaxHQ holds has stopped, as the desktop
// has it: you stopped it, it kept making no progress, or its CLI exited.
// Resume starts it again on the same conversation, and so does sending it
// a message. A local ACP agent that needs a sign-in gets the sign-in card
// instead: the ways it says it signs in, its command to copy for a terminal
// on its device, and Try again.

import { useI18n } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import { useState } from "react";
import { Alert, StyleSheet, Text, View } from "react-native";

import { failureOf, type SendState } from "@maxhq/shared/agent-view/answer";
import { resumeWording, showsResume } from "@maxhq/shared/agent-view/resume";
import type { Agent } from "@maxhq/shared/bindings";
import { agentOfAccount, signInCommand } from "@maxhq/shared/settings/acp-agents";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";

export const ResumeBanner = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [state, setState] = useState<SendState>({ state: "idle" });
	const account = useView((s) =>
		agent.account ? s.view.tables.account.get(agent.account) : undefined
	);
	const definition = useView((s) => (account ? agentOfAccount(s.view, account) : undefined));
	const where = useView((s) => (account ? deviceName(s.view, account.device) : ""));
	// One that moved away has the moved banner instead.
	if (agent.moved?.to || !showsResume(agent, account)) return null;
	const { signIn, title, body, methods } = resumeWording(agent, account, where);
	const command = signIn && definition ? signInCommand(definition) : null;

	const resume = () => {
		setState({ state: "sending" });
		source.continueHere(agent.id).then(
			(result) => {
				const failure = failureOf(result);
				setState(failure ? { state: "failed", message: failure } : { state: "sent" });
			},
			(error: unknown) =>
				setState({
					state: "failed",
					message:
						error instanceof Error && error.message
							? error.message
							: t("Couldn't resume the agent."),
				})
		);
	};
	const copy = (text: string) =>
		void Clipboard.setStringAsync(text).then(
			() => Alert.alert(t("Copied"), text),
			() => Alert.alert(t("Couldn't copy"))
		);

	return (
		<View style={styles.bar}>
			<View
				style={styles.box}
				accessibilityLabel={signIn ? t("Agent needs a sign-in") : t("Agent stopped")}
			>
				<Text style={styles.title}>{title}</Text>
				<Text style={styles.body}>{body}</Text>
				{methods.map((method) => (
					<Text key={method.id} style={styles.body}>
						<Text style={styles.method}>{method.name}</Text>
						{method.description ? `: ${method.description}` : null}
					</Text>
				))}
				{command ? (
					<Text selectable style={styles.code}>
						{command}
					</Text>
				) : null}
				{state.state === "failed" ? (
					<Text style={styles.failed} accessibilityRole="alert">
						{state.message}
					</Text>
				) : null}
				<View style={styles.actions}>
					{command ? (
						<Button label={t("Copy command")} small onPress={() => copy(command)} />
					) : null}
					<Button
						label={signIn ? t("Try again") : t("Resume")}
						tone="primary"
						small
						disabled={state.state === "sending" || state.state === "sent"}
						onPress={resume}
					/>
				</View>
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	bar: { backgroundColor: color.background, paddingHorizontal: 10, paddingVertical: 8 },
	box: {
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
		borderRadius: radius.lg,
		backgroundColor: color.card,
		padding: 12,
		gap: 6,
	},
	title: { color: color.foregroundStrong, fontSize: size.small, fontWeight: "600" },
	body: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	method: { color: color.secondaryForeground, fontWeight: "500" },
	code: { fontFamily: font.mono, fontSize: size.tiny, color: color.terminal },
	failed: { color: color.errorText, fontSize: size.meta },
	actions: { flexDirection: "row", justifyContent: "flex-end", gap: 8 },
});
