// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An agent's details, which the second line of its header opens: what the
// desktop's agent header shows beside the title, laid out as rows. Its
// account and model, how full its context window is, its folder and branch,
// the ID its CLI resumes the session by, who started it, where it moved,
// whether it runs in a terminal, whether changed skills restart it, and
// whether it may run agents, which the phone can revoke but only the Mac
// can allow. Its sub-agents and its ports and processes open from here too.

import { formatNumber, t, useI18n } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import { useEffect } from "react";
import { Alert, ScrollView, StyleSheet, Text, View } from "react-native";
import { useStore } from "zustand";

import { canAllow, limitsLine } from "@maxhq/shared/agent-tools/scope";
import { modelLabel, sessionIdName } from "@maxhq/shared/agent-view/controls";
import { movedBanner } from "@maxhq/shared/agent-view/move-agent";
import type { Agent } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { waitsForSkills } from "@maxhq/shared/skills/library";
import { ACTIVITY_LABEL, agentLook } from "@maxhq/shared/status/activity";
import { formatContext } from "@maxhq/shared/status/format";
import { contextShare } from "@maxhq/shared/status/sub-agents";
import { accountName, VENDOR_NAME } from "@maxhq/shared/status/usage";
import { deviceName, modelsOf, startedByLine } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, radius, size, TOUCH } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { ActivityDot } from "../../ui/Dot";
import { Copy } from "../../ui/glyphs";
import { Header, IconButton } from "../../ui/Header";
import { askRevoke } from "./AgentMore";
import { loadPermissions, permissions, useHeldPermission } from "./permissions";
import { useShownTitle } from "./renames";
import { openPortCount, showsPorts } from "./running";
import { hasSubAgents, subAgentCount } from "./sub-agents";

/** Copies `text`, and says so. */
const copy = (text: string) =>
	void Clipboard.setStringAsync(text).then(
		() => Alert.alert(t("Copied"), text),
		() => Alert.alert(t("Couldn't copy"))
	);

/**
 * A long value, such as a path, under its label, as a card row lays out
 * its note, with a button that copies it when `copyLabel` is given.
 */
const ValueRow = ({
	label,
	value,
	copyLabel,
}: {
	label: string;
	value: string;
	copyLabel?: string;
}) => (
	<View style={styles.stack}>
		<View style={styles.stackText}>
			<Text style={styles.label}>{label}</Text>
			<Text selectable style={styles.mono}>
				{value}
			</Text>
		</View>
		{copyLabel ? (
			<IconButton label={copyLabel} onPress={() => copy(value)}>
				<Copy size={15} strokeWidth={2} color={color.secondaryForeground} />
			</IconButton>
		) : null}
	</View>
);

/** How full the context window is: a bar and the tokens used of the window. */
const ContextRow = ({ context }: { context: NonNullable<Agent["context"]> }) => {
	const { t } = useI18n();
	const percent = contextShare(context) ?? 0;
	const text = formatContext(context.used_tokens, context.window_tokens);
	return (
		<CardRow label={t("Context")}>
			<View
				accessible
				accessibilityRole="progressbar"
				accessibilityLabel={t("Context window, {{text}} used", { text })}
				accessibilityValue={{ min: 0, max: 100, now: Math.round(percent) }}
				style={styles.meter}
			>
				<View style={styles.track}>
					<View
						style={[
							styles.fill,
							{ width: `${percent}%` },
							percent >= 85 && styles.full,
						]}
					/>
				</View>
				<Text style={styles.value}>{text}</Text>
			</View>
		</CardRow>
	);
};

/** Whether it may run agents: what it may run, with Revoke, or that the Mac allows it. */
const AllowedCard = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const read = useStore(permissions, (s) => s.list !== null);
	const held = useHeldPermission(agent);
	const now = useNow(30_000);
	return (
		<Card>
			<CardRow
				first
				label={t("Can run agents")}
				note={
					held
						? limitsLine(held.limits, now)
						: read
							? t("Not allowed. Allow it on the Mac.")
							: t("Reading permissions")
				}
			>
				{held ? (
					<Button
						label={t("Revoke")}
						tone="danger"
						small
						onPress={() => askRevoke(source, held)}
					/>
				) : null}
			</CardRow>
		</Card>
	);
};

export const AgentInfoScreen = ({ navigation, route }: ScreenProps<"AgentInfo">) => {
	const { t } = useI18n();

	const agent = useView((s) => s.view.tables.agent.get(route.params.agent));
	const back = { label: t("Back"), onPress: () => navigation.goBack() };
	if (agent) return <Info agent={agent} back={back} navigation={navigation} />;
	return (
		<View style={styles.screen}>
			<Header title={t("Details")} back={back} />
			<Text style={styles.gone}>{t("This agent is gone.")}</Text>
		</View>
	);
};

const Info = ({
	agent,
	back,
	navigation,
}: {
	agent: Agent;
	back: { label: string; onPress: () => void };
	navigation: ScreenProps<"AgentInfo">["navigation"];
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(10_000);
	const title = useShownTitle(agent);
	const allows = canAllow(agent);
	useEffect(() => {
		if (allows) void loadPermissions(permissions, source);
	}, [allows, source]);

	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const workspace = view.tables.workspace.get(agent.workspace);
	const device = workspace ? deviceName(view, workspace.device) : t("this device");
	const project = workspace?.project
		? view.tables.project.get(workspace.project)?.name
		: undefined;
	const look = agentLook(agent, now);
	const place = [
		ACTIVITY_LABEL[look],
		t("{{vendor}} on {{device}}", { vendor: VENDOR_NAME[agent.vendor], device }),
		...(project ? [project] : []),
	].join(", ");
	const session = agent.vendor_session_id;
	const idName = sessionIdName(agent);
	const startedBy = startedByLine(view, agent);
	const moved = movedBanner(view, agent);
	const other = moved?.other;
	const terminal =
		agent.control === "observed"
			? agent.link.state === "down"
				? t("Ended in a terminal")
				: t("In a terminal, read only")
			: null;
	const skills = waitsForSkills(view, agent.id);
	const subAgents = hasSubAgents(view, agent.id) ? subAgentCount(view, agent.id) : null;
	const ports = showsPorts(view, agent) ? openPortCount(view, agent.id) : null;
	const notes = Boolean(startedBy || moved || terminal || skills);

	return (
		<View style={styles.screen}>
			<Header
				title={title}
				back={back}
				subtitle={
					<View style={styles.place}>
						<ActivityDot look={look} decorative />
						<Text style={styles.placeText} numberOfLines={1}>
							{place}
						</Text>
					</View>
				}
			/>
			<ConnectionBanner />
			<ScrollView contentContainerStyle={styles.content}>
				<Card label={t("Details")}>
					<CardRow first label={t("Account")}>
						<Text style={styles.value} numberOfLines={1}>
							{account ? accountName(account) : VENDOR_NAME[agent.vendor]}
						</Text>
					</CardRow>
					{agent.model ? (
						<CardRow label={t("Model")}>
							<Text style={styles.value} numberOfLines={1}>
								{modelLabel(agent.model, modelsOf(view, account))}
							</Text>
						</CardRow>
					) : null}
					{agent.context ? <ContextRow context={agent.context} /> : null}
					{workspace?.location ? (
						<ValueRow label={t("Folder")} value={workspace.location.path} />
					) : null}
					{agent.branch ? (
						<ValueRow
							label={t("Branch")}
							value={agent.branch}
							copyLabel={t("Copy branch {{branch}}", { branch: agent.branch })}
						/>
					) : null}
					{session ? (
						<ValueRow
							label={idName.charAt(0).toLocaleUpperCase() + idName.slice(1)}
							value={session}
							copyLabel={t("Copy {{sessionIdName}}", { sessionIdName: idName })}
						/>
					) : null}
				</Card>
				{notes ? (
					<Card>
						{startedBy ? <CardRow first label={startedBy} /> : null}
						{moved ? (
							<CardRow
								first={!startedBy}
								label={moved.title}
								note={moved.detail}
								onPress={
									other
										? () => navigation.push("Agent", { agent: other })
										: undefined
								}
							/>
						) : null}
						{terminal ? (
							<CardRow first={!startedBy && !moved} label={terminal} />
						) : null}
						{skills ? (
							<CardRow
								first={!startedBy && !moved && !terminal}
								label={t("Skills changed, restarts on next message")}
							/>
						) : null}
					</Card>
				) : null}
				{allows ? <AllowedCard agent={agent} /> : null}
				{subAgents !== null || ports !== null ? (
					<Card>
						{subAgents !== null ? (
							<CardRow
								first
								label={t("Sub-agents")}
								onPress={() =>
									navigation.navigate("SubAgents", { agent: agent.id })
								}
							>
								<Text style={styles.count}>{formatNumber(subAgents)}</Text>
							</CardRow>
						) : null}
						{ports !== null ? (
							<CardRow
								first={subAgents === null}
								label={t("Ports and processes")}
								onPress={() => navigation.navigate("Running", { agent: agent.id })}
							>
								{ports > 0 ? (
									<Text style={styles.count}>{formatNumber(ports)}</Text>
								) : null}
							</CardRow>
						) : null}
					</Card>
				) : null}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 12, paddingBottom: 40, gap: 12 },
	gone: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
	place: { flexDirection: "row", alignItems: "center", gap: 6, marginTop: 1 },
	placeText: { flex: 1, color: color.mutedForeground, fontSize: size.meta },
	value: {
		flexShrink: 1,
		color: color.secondaryForeground,
		fontSize: size.small,
		textAlign: "right",
	},
	stack: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingLeft: 14,
		paddingRight: 4,
		paddingVertical: 8,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	stackText: { flex: 1, gap: 2 },
	label: { color: color.foreground, fontSize: size.small },
	mono: { fontFamily: font.mono, fontSize: size.tiny, color: color.terminal },
	count: { color: color.mutedForeground, fontSize: size.small, fontVariant: ["tabular-nums"] },
	meter: { flexDirection: "row", alignItems: "center", gap: 8 },
	track: {
		width: 48,
		height: 4,
		borderRadius: radius.sm,
		overflow: "hidden",
		backgroundColor: color.track,
	},
	fill: { height: 4, backgroundColor: color.primary },
	full: { backgroundColor: color.busy },
});
