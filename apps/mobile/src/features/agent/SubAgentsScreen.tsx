// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A lead's sub-agents, as the desktop's slide-out lists them: the lead
// first, then each sub-agent with its dot and what it's doing, then how full
// the lead's context window is. A row opens that agent. The screen follows
// only the view's agents, and each row only draws again when what it shows
// changed.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useCallback, useMemo } from "react";
import { FlatList, type ListRenderItem, Pressable, StyleSheet, Text, View } from "react-native";

import { keepCardSubAgent } from "@maxhq/shared/agent-view/finished";
import type { AgentId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import type { ActivityLook } from "@maxhq/shared/status/activity";
import { formatContext } from "@maxhq/shared/status/format";
import { contextShare, subAgentsSubtitle } from "@maxhq/shared/status/sub-agents";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, radius, size } from "../../theme";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { ActivityDot } from "../../ui/Dot";
import { Header } from "../../ui/Header";
import { type SubAgentRow, subAgentRows } from "./sub-agents";

/** One agent's row: its dot, its title, and what it's doing. */
const Row = memo(
	({
		id,
		title,
		look,
		status,
		lead,
		quiet,
		onOpen,
	}: {
		id: AgentId;
		title: string;
		look: ActivityLook;
		status: string;
		lead: boolean;
		quiet: boolean;
		onOpen: (agent: AgentId, lead: boolean) => void;
	}) => {
		const { t } = useI18n();

		const name = lead ? t("Lead") : title;
		return (
			<Pressable
				accessibilityRole="button"
				onPress={() => onOpen(id, lead)}
				style={({ pressed }) => [
					styles.row,
					lead && styles.leadRow,
					pressed && styles.pressed,
				]}
			>
				<ActivityDot
					look={look}
					surface={lead ? color.secondary : color.background}
					decorative
				/>
				<Text
					numberOfLines={1}
					style={[styles.title, lead ? styles.leadTitle : quiet ? styles.quiet : null]}
				>
					{name}
				</Text>
				<Text numberOfLines={1} style={styles.status}>
					{status}
				</Text>
			</Pressable>
		);
	}
);
Row.displayName = "SubAgentRow";

/** How full the lead's context window is, under the list. */
const ContextMeter = ({ used, window }: { used: number; window: number }) => {
	const { t } = useI18n();

	const percent = contextShare({ used_tokens: used, window_tokens: window });
	if (percent === null) return null;
	return (
		<View style={styles.context}>
			<View style={styles.contextTop}>
				<Text style={styles.contextLabel}>{t("Context")}</Text>
				<Text style={styles.contextValue}>{formatContext(used, window)}</Text>
			</View>
			<View
				accessible
				accessibilityRole="progressbar"
				accessibilityLabel={t("Lead's context window")}
				accessibilityValue={{ min: 0, max: 100, now: Math.round(percent) }}
				style={styles.track}
			>
				<View style={[styles.fill, { width: `${percent}%` }]} />
			</View>
		</View>
	);
};

export const SubAgentsScreen = ({ navigation, route }: ScreenProps<"SubAgents">) => {
	const { t } = useI18n();

	const { view: store } = useServices();
	// The agents alone, which keep their map while other parts of the view change.
	const agents = useView((s) => s.view.tables.agent);
	const now = useNow(10_000);
	const id = route.params.agent;
	const lead = agents.get(id);
	const rows = useMemo(
		() => (lead ? subAgentRows(store.getState().view, lead, now) : []),
		// The rows read only the agents, so a new agents map is what makes new rows.
		// eslint-disable-next-line react-hooks/exhaustive-deps
		[store, agents, lead, now]
	);
	const count = rows.length - 1;
	const subtitle = useView((s) => (lead ? subAgentsSubtitle(s.view, lead, count) : ""));

	const onOpen = useCallback(
		(agent: AgentId, isLead: boolean) => {
			if (isLead) {
				navigation.navigate("Agent", { agent });
				return;
			}
			const view = store.getState().view;
			const title = view.tables.agent.get(agent)?.title ?? "";
			// Kept under the lead, so the sub-agent still opens once it finished.
			keepCardSubAgent(view, id, agent, title);
			navigation.push("Agent", { agent });
		},
		[navigation, store, id]
	);
	const renderItem = useCallback<ListRenderItem<SubAgentRow>>(
		({ item }) => (
			<Row
				id={item.id}
				title={item.title}
				look={item.look}
				status={item.status}
				lead={item.lead}
				quiet={item.quiet}
				onOpen={onOpen}
			/>
		),
		[onOpen]
	);

	const back = { label: t("Back"), onPress: () => navigation.goBack() };
	if (!lead) {
		return (
			<View style={styles.screen}>
				<Header title={t("Sub-agents")} back={back} />
				<Text style={styles.gone}>{t("This agent is gone.")}</Text>
			</View>
		);
	}

	const context = lead.context;
	return (
		<View style={styles.screen}>
			<Header
				title={t("Sub-agents")}
				back={back}
				subtitle={
					<Text numberOfLines={2} style={styles.subtitle}>
						{subtitle}
					</Text>
				}
			/>
			<ConnectionBanner />
			<FlatList
				data={rows}
				keyExtractor={(row) => row.id}
				renderItem={renderItem}
				contentContainerStyle={styles.list}
				initialNumToRender={20}
				ListFooterComponent={
					context ? (
						<ContextMeter used={context.used_tokens} window={context.window_tokens} />
					) : undefined
				}
			/>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	subtitle: { color: color.mutedForeground, fontSize: size.meta, marginTop: 1 },
	gone: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
	list: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 40, gap: 1 },
	row: {
		height: 40,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 10,
		borderRadius: radius.md,
	},
	leadRow: { backgroundColor: color.secondary },
	pressed: { backgroundColor: color.accent },
	title: { flex: 1, minWidth: 0, color: color.terminal, fontSize: size.small },
	leadTitle: { color: color.foreground },
	quiet: { color: color.mutedForeground },
	status: { flexShrink: 0, color: color.mutedForeground, fontSize: size.meta },
	context: {
		marginTop: 12,
		marginHorizontal: 10,
		paddingTop: 10,
		gap: 6,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	contextTop: { flexDirection: "row", justifyContent: "space-between", alignItems: "center" },
	contextLabel: { color: color.mutedForeground, fontSize: size.meta },
	contextValue: {
		color: color.secondaryForeground,
		fontSize: size.meta,
		fontVariant: ["tabular-nums"],
	},
	track: { height: 4, borderRadius: 2, overflow: "hidden", backgroundColor: color.track },
	fill: { height: 4, borderRadius: 2, backgroundColor: color.primary },
});
