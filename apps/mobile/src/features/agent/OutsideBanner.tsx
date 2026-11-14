// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { StyleSheet, Text, View } from "react-native";

import type { Agent } from "@maxhq/shared/bindings";
import {
	MANUAL_NOTE,
	movedNote,
	type NotePart,
	outsideNote,
	outsideOffer,
	rootLead,
	takeOverQuestion,
	takingOverNote,
	useContinueHere,
	useFollowLead,
	useTakeOver,
} from "@maxhq/shared/agent-view/outside";
import { useNow } from "@maxhq/shared/lib/clock";
import { childrenOf, linkForDevice } from "@maxhq/shared/view/select";

import type { Screens } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { choose, confirm } from "../../ui/choose";
import { continueTone } from "./compose";

const Parts = ({ parts }: { parts: readonly NotePart[] }) => {
	return parts.map((part, i) =>
		part.code ? (
			<Text key={i} style={styles.code}>
				{part.text}
			</Text>
		) : (
			part.text
		)
	);
};

/**
 * Where a sub-agent's session went once it continues in MaxHQ as `lead`, in
 * its banner or, once the sub-agent is gone, on its screen.
 */
export const SessionMoved = ({ lead }: { lead: Agent }) => {
	const { t } = useI18n();
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const { title, body } = movedNote(lead);
	return (
		<View style={styles.box} accessibilityLabel={t("Session continues in MaxHQ")}>
			<Text style={styles.title}>{title}</Text>
			<Text style={styles.body}>{body}</Text>
			<Button
				label={t("Open session")}
				tone="primary"
				small
				onPress={() => navigation.navigate("Agent", { agent: lead.id })}
			/>
		</View>
	);
};

/**
 * In place of the composer for a session started in a terminal on `device`,
 * or a sub-agent in one, as the desktop has it.
 */
export const OutsideBanner = ({ agent, device }: { agent: Agent; device: string }) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const link = useView((s) => {
		const workspace = s.view.tables.workspace.get(agent.workspace);
		return (workspace && linkForDevice(s.view, workspace.device)?.state) ?? null;
	});
	const protocol = useView((s) => {
		const workspace = s.view.tables.workspace.get(agent.workspace);
		return (workspace && linkForDevice(s.view, workspace.device)?.protocol) ?? null;
	});
	// A sub-agent's session is its lead's, at the top of its tree.
	const lead = useView((s) => rootLead(s.view.tables.agent, agent));
	useFollowLead(agent.id, lead?.id ?? null);
	// For "12 min ago" while the device can't be reached.
	const now = useNow(30_000);
	const [state, continueHere] = useContinueHere(source, agent.id);
	const [taken, takeOver] = useTakeOver(source, lead?.id ?? agent.id);
	const note = outsideNote(agent, device, { link, now });
	const offer = outsideOffer(agent, lead, note, protocol, link, device);
	if (offer.kind === "moved")
		return (
			<View style={styles.bar}>
				<SessionMoved lead={offer.lead} />
			</View>
		);
	const busy = state.state === "sending" || state.state === "sent";
	const failed = [state, taken].find((sent) => sent.state === "failed");
	const ask = () => {
		if (!lead) return;
		const subAgents = childrenOf(services.view.getState().view, lead.id).length > 0;
		const question = takeOverQuestion(agent, device, lead, subAgents);
		if (!question.alternative) return confirm(question, question.action, () => takeOver("now"));
		choose(
			question.title,
			[
				{ label: question.action, pick: () => takeOver("after_turn") },
				{ label: question.alternative, pick: () => takeOver("now"), destructive: true },
			],
			question.detail
		);
	};
	// Why the button is off, or how to leave the terminal by hand.
	const below =
		offer.kind === "off"
			? offer.why
			: offer.kind === "take_over" && agent.parent === null
				? MANUAL_NOTE
				: null;

	return (
		<View style={styles.bar}>
			<View
				style={styles.box}
				accessibilityLabel={
					note.ended ? t("Terminal session ended") : t("Running in a terminal")
				}
			>
				<Text style={styles.title}>{note.title}</Text>
				<Text style={styles.body}>
					<Parts parts={note.body} />
				</Text>
				{offer.kind === "taking" && lead ? (
					<Text style={styles.body}>{takingOverNote(agent, device, lead)}</Text>
				) : null}
				{failed?.state === "failed" ? (
					<Text style={styles.failed} accessibilityRole="alert">
						{failed.message}
					</Text>
				) : null}
				{offer.kind === "taking" ? (
					<Button
						label={t("Cancel")}
						small
						disabled={taken.state === "sending"}
						onPress={() => takeOver("cancel")}
					/>
				) : offer.kind === "take_over" ? (
					<Button
						label={t("Take over")}
						tone={continueTone(true)}
						small
						disabled={taken.state === "sending"}
						onPress={ask}
					/>
				) : (
					<Button
						label={
							offer.kind === "off" && offer.button === "take_over"
								? t("Take over")
								: t("Continue in MaxHQ")
						}
						tone={continueTone(offer.kind === "continue")}
						small
						disabled={offer.kind === "off" || busy}
						onPress={continueHere}
					/>
				)}
				{below ? (
					<Text style={styles.wait}>
						<Parts parts={below} />
					</Text>
				) : null}
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
	code: { fontFamily: font.mono, fontSize: size.tiny, color: color.terminal },
	wait: { color: color.subtleForeground, fontSize: size.tiny, lineHeight: 16 },
	failed: { color: color.errorText, fontSize: size.meta },
});
