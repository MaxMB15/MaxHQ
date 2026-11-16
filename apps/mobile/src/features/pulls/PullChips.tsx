// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The PR chips on an agent's screen: one per PR linked to it, its number
// colored by the PR's state with a dot for its checks, in a strip that
// scrolls sideways when they don't fit. A chip opens the PR's screen, and
// its arrow opens a sheet that opens the PR on GitHub, sends the agent a
// message about it after the Auto mode check the composer makes, unlinks
// it or links another. A control that fails says why in an alert.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { useState } from "react";
import { Alert, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import type { Agent, PullLink } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import {
	type ChipChecks,
	chipChecks,
	chipLabel,
	type ChipState,
	chipState,
} from "@maxhq/shared/pulls/chip";
import { shownPulls } from "@maxhq/shared/pulls/links";
import { pullUrl } from "@maxhq/shared/pulls/urls";
import { agreeToSend } from "@maxhq/shared/settings/auto-mode";

import type { Screens } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size, tint } from "../../theme";
import { choose } from "../../ui/choose";
import { ChevronDown, GitPullRequest } from "../../ui/glyphs";
import { askAutoDefault, autoConsent } from "../agent/auto-consent";
import { runControl } from "../agent/more";
import { LinkPullSheet } from "./LinkPullSheet";
import { openOut } from "./look";
import { agentDevice, chipActions, pullId } from "./pulls";

/** What a message you chose not to send comes back with: nothing to say. */
const LEFT: ActionResult = { status: "ok", data: { status: "done" } };

// The hues the desktop's chips use, so a PR looks the same everywhere.
const STATE_LOOK: Record<ChipState, { box: string; border: string; text: string }> = {
	open: { box: tint(color.ready, 0.14), border: tint(color.ready, 0.35), text: color.readyText },
	draft: {
		box: tint(color.mutedForeground, 0.14),
		border: tint(color.mutedForeground, 0.35),
		text: color.secondaryForeground,
	},
	merged: { box: tint(color.linked, 0.14), border: tint(color.linked, 0.35), text: color.linked },
	closed: {
		box: tint(color.error, 0.14),
		border: tint(color.error, 0.35),
		text: color.errorText,
	},
	unread: { box: color.secondary, border: color.track, text: color.secondaryForeground },
};

const DOT_LOOK: Record<ChipChecks, string> = {
	passing: color.ready,
	failing: color.error,
	pending: color.busy,
	unknown: color.off,
};

// The chip is 26 tall, and its slop takes a touch to 44.
const SLOP = { top: 9, bottom: 9 };

const PullChip = ({
	agent,
	link,
	onLink,
}: {
	agent: Agent;
	link: PullLink;
	onLink: () => void;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const device = useView((s) => agentDevice(s.view, agent));
	const look = STATE_LOOK[chipState(link)];
	const checks = chipChecks(link);
	const { repo, number } = link.key;
	const shown = formatNumber(number, { useGrouping: false });

	const run = (what: string, action: () => Promise<ActionResult>) =>
		void runControl(what, action).then((why) => {
			if (why) Alert.alert(why);
		});
	// An Auto you didn't pick asks first. An agent that couldn't leave it says why.
	const send = (text: string) =>
		run(t("send the message"), async () => {
			const view = services.view.getState().view;
			const { go, failure } = await agreeToSend(
				agent,
				autoConsent,
				askAutoDefault,
				source,
				view
			);
			if (failure && !go) throw new Error(failure);
			if (failure) console.warn(failure);
			if (!go) return LEFT;
			return source.sendInput(agent.id, { text, attachments: [] }, "queue");
		});
	const menu = () =>
		choose(
			[`#${shown}`, link.snapshot?.title].filter(Boolean).join(" "),
			chipActions(agent, link).map((action) => {
				switch (action.kind) {
					case "github":
						return {
							label: t("Open on GitHub"),
							pick: () => openOut(pullUrl(link.key)),
						};
					case "prompt":
						return { label: action.prompt.label, pick: () => send(action.prompt.text) };
					case "unlink":
						return {
							label: t("Unlink"),
							pick: () =>
								run(t("unlink the PR"), () =>
									source.unlinkPull(agent.id, repo, number)
								),
						};
					case "link":
						return { label: t("Link another PR"), pick: onLink };
				}
			})
		);

	const box = { backgroundColor: look.box, borderColor: look.border };
	return (
		<View style={styles.chip}>
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={chipLabel(link)}
				accessibilityState={{ disabled: !device }}
				disabled={!device}
				hitSlop={SLOP}
				onPress={() => device && navigation.navigate("Pull", { device, repo, number })}
				style={({ pressed }) => [styles.body, box, pressed && styles.pressed]}
			>
				<GitPullRequest size={11} strokeWidth={2.2} color={look.text} />
				<Text style={[styles.number, { color: look.text }]}>#{shown}</Text>
				{checks ? (
					<View style={[styles.dot, { backgroundColor: DOT_LOOK[checks] }]} />
				) : null}
			</Pressable>
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={t("More for pull request {{number}}", { number: shown })}
				hitSlop={{ ...SLOP, right: 4 }}
				onPress={menu}
				style={({ pressed }) => [styles.arrow, box, pressed && styles.pressed]}
			>
				<ChevronDown size={11} strokeWidth={2.4} color={look.text} />
			</Pressable>
		</View>
	);
};

/**
 * The agent's PR chips, or nothing when it has no PR linked. They scroll
 * sideways when they don't fit, so each keeps its sheet.
 */
export const PullChips = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();

	const [linking, setLinking] = useState(false);
	const links = shownPulls(agent);
	if (links.length === 0) return null;
	return (
		<>
			<ScrollView
				horizontal
				showsHorizontalScrollIndicator={false}
				accessibilityLabel={t("Pull requests")}
				contentContainerStyle={styles.strip}
				style={styles.scroll}
			>
				{links.map((link) => (
					<PullChip
						key={pullId(link.key)}
						agent={agent}
						link={link}
						onLink={() => setLinking(true)}
					/>
				))}
			</ScrollView>
			<LinkPullSheet agent={agent} open={linking} onClose={() => setLinking(false)} />
		</>
	);
};

const styles = StyleSheet.create({
	scroll: { flexGrow: 0 },
	strip: { gap: 6, paddingHorizontal: 14, paddingVertical: 9 },
	chip: { flexDirection: "row", alignItems: "center" },
	body: {
		height: 26,
		flexDirection: "row",
		alignItems: "center",
		gap: 5,
		paddingLeft: 8,
		paddingRight: 5,
		borderWidth: StyleSheet.hairlineWidth,
		borderRightWidth: 0,
		borderTopLeftRadius: 6,
		borderBottomLeftRadius: 6,
	},
	arrow: {
		height: 26,
		justifyContent: "center",
		paddingLeft: 4,
		paddingRight: 7,
		borderWidth: StyleSheet.hairlineWidth,
		borderLeftWidth: 0,
		borderTopRightRadius: 6,
		borderBottomRightRadius: 6,
	},
	number: { fontSize: size.tiny, fontWeight: "600", fontVariant: ["tabular-nums"] },
	dot: { width: 6, height: 6, borderRadius: 3 },
	pressed: { opacity: 0.7 },
});
