// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A sub-agent the agent started: what it was asked, how many tool calls it
// has made, how long it has run and the step it's on, and a button that opens
// it on its own screen. Its report opens below.

import { useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { useCallback } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { useFold } from "@maxhq/shared/agent-view/find-context";
import { keepCardSubAgent } from "@maxhq/shared/agent-view/finished";
import { isOpen } from "@maxhq/shared/agent-view/format";
import {
	endedLine,
	everything,
	runningLine,
	useWork,
	type Work,
} from "@maxhq/shared/agent-view/subagent";
import type { AgentId, TimelineItem } from "@maxhq/shared/bindings";
import { sentence } from "@maxhq/shared/status/format";
import { useTimeline } from "@maxhq/shared/timeline/use-timeline";

import type { Screens } from "../../navigation";
import { useServices } from "../../services";
import { color, font, radius, size, tint } from "../../theme";
import { Spinner } from "../../ui/Dot";
import { Elapsed } from "../../ui/Elapsed";
import { ChevronDown, ChevronRight, CircleCheck, CircleX } from "../../ui/glyphs";
import { MarkdownText } from "../../ui/MarkdownText";
import { useFeed } from "./feed-context";

type Item = Extract<TimelineItem, { kind: "subagent" }>;

const Meta = ({ item, work }: { item: Item; work: Work }) => {
	const { t } = useI18n();

	if (!isOpen(item.status)) {
		return (
			<Text numberOfLines={1} style={styles.meta}>
				{endedLine(item, work)}
			</Text>
		);
	}
	const { words, since } = runningLine(item.started_at, work);
	return (
		<Text numberOfLines={1} style={styles.meta}>
			{words}
			{since !== null ? <Elapsed since={since} /> : null}
			{work.now !== null ? (
				<>
					{t(". Now: ")}
					<Text style={styles.now}>{work.now}</Text>
				</>
			) : null}
		</Text>
	);
};

// A sub-agent with a timeline of its own.
const OwnMeta = ({ agent, item }: { agent: AgentId; item: Item }) => {
	const { source } = useServices();
	const timeline = useTimeline(source, agent);
	const work = useWork(timeline.store, everything, isOpen(item.status), true);
	return <Meta item={item} work={work} />;
};

// A sub-agent whose steps sit after its card in the lead's timeline.
const NestedMeta = ({ item }: { item: Item }) => {
	const { store } = useFeed();
	const mine = useCallback((other: TimelineItem) => other.parent === item.id, [item.id]);
	return <Meta item={item} work={useWork(store, mine, isOpen(item.status), false)} />;
};

const OpenButton = ({ agent, title }: { agent: AgentId; title: string }) => {
	const { t } = useI18n();

	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const services = useServices();
	const feed = useFeed();
	const open = () => {
		// So it still opens, by this name, once it finished.
		keepCardSubAgent(services.view.getState().view, feed.agent, agent, title);
		navigation.push("Agent", { agent });
	};
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={t("Open this sub-agent")}
			hitSlop={9}
			onPress={open}
			style={({ pressed }) => [styles.open, pressed ? styles.pressed : null]}
		>
			<Text style={styles.openText}>{t("Open")}</Text>
		</Pressable>
	);
};

/** A sub-agent card. */
export const SubAgentCard = ({ item }: { item: Item }) => {
	const { t } = useI18n();

	// While find's active match is in it, the row shows its whole description and its report.
	const [open, setOpen, held] = useFold(`open:${item.id}`, item.id);
	const running = isOpen(item.status);
	const hasResult = Boolean(item.result?.trim());
	const Chevron = open ? ChevronDown : ChevronRight;
	const summary = (
		<>
			{running ? (
				<View style={styles.spinning}>
					<Spinner size={10} />
				</View>
			) : item.status === "failed" ? (
				<CircleX size={14} strokeWidth={1.9} color={color.destructive} />
			) : (
				<CircleCheck size={14} strokeWidth={1.9} color={color.ready} />
			)}
			<View style={styles.words}>
				<Text numberOfLines={held ? undefined : 1} style={styles.what}>
					<Text style={styles.type}>
						{item.agent_type ? sentence(item.agent_type) : t("Task")}:
					</Text>{" "}
					{item.description}
				</Text>
				{item.subagent ? (
					<OwnMeta agent={item.subagent} item={item} />
				) : (
					<NestedMeta item={item} />
				)}
			</View>
			{hasResult ? (
				<Chevron size={10} strokeWidth={2.4} color={color.mutedForeground} />
			) : null}
		</>
	);
	return (
		<View style={styles.step}>
			<View style={styles.card}>
				{hasResult ? (
					<Pressable
						accessibilityRole="button"
						accessibilityState={{ expanded: open }}
						accessibilityHint={open ? t("Hides its report") : t("Shows its report")}
						onPress={() => setOpen(!open)}
						style={styles.summary}
					>
						{summary}
					</Pressable>
				) : (
					<View style={styles.summary}>{summary}</View>
				)}
				{item.subagent ? (
					<OpenButton agent={item.subagent} title={item.description} />
				) : null}
			</View>
			{open && hasResult ? (
				<View style={styles.report}>
					<MarkdownText text={item.result ?? ""} quiet={color.secondaryForeground} />
				</View>
			) : null}
		</View>
	);
};

const styles = StyleSheet.create({
	step: { gap: 6 },
	card: {
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingVertical: 9,
		paddingLeft: 12,
		paddingRight: 10,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.primary, 0.35),
		backgroundColor: color.subpanel,
	},
	summary: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", gap: 10 },
	spinning: { width: 14, alignItems: "center" },
	words: { flex: 1, minWidth: 0, gap: 2 },
	what: { color: color.foreground, fontSize: 12.5 },
	type: { fontWeight: "600" },
	meta: { color: color.mutedForeground, fontSize: size.tiny, fontVariant: ["tabular-nums"] },
	now: { color: color.secondaryForeground, fontFamily: font.mono, fontSize: size.pill },
	open: {
		height: 26,
		justifyContent: "center",
		paddingHorizontal: 10,
		borderRadius: 6,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	openText: { color: color.foreground, fontSize: size.meta },
	report: { marginLeft: 19, paddingLeft: 16, borderLeftWidth: 2, borderLeftColor: color.track },
});
