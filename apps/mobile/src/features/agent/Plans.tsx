// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A plan the agent wrote, and its to-do list with a bar for each step.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import type { TimelineItem, TodoState } from "@maxhq/shared/bindings";

import { color, radius, size, tint } from "../../theme";
import { Spinner } from "../../ui/Dot";
import { CircleCheck, ListChecks, ListTodo } from "../../ui/glyphs";
import { withCaret } from "../../ui/markdown";
import { MarkdownText } from "../../ui/MarkdownText";

type ItemOf<K extends TimelineItem["kind"]> = Extract<TimelineItem, { kind: K }>;

const ICON = { size: 14, strokeWidth: 1.9, color: color.mutedForeground } as const;

/** A plan the agent proposed, as Markdown. */
export const PlanCard = ({ item }: { item: ItemOf<"plan"> }) => {
	const { t } = useI18n();
	return (
		<View accessibilityLabel={t("Proposed plan")} style={[styles.card, styles.plan]}>
			<View style={styles.head}>
				<ListTodo {...ICON} />
				<Text style={styles.title}>{t("Proposed plan")}</Text>
				{item.streaming ? (
					<View style={styles.writing}>
						<Spinner size={10} />
						<Text style={styles.writingText}>{t("Writing")}</Text>
					</View>
				) : null}
			</View>
			<MarkdownText text={item.streaming ? withCaret(item.markdown) : item.markdown} />
		</View>
	);
};

const BAR: Record<TodoState, string> = {
	done: color.ready,
	in_progress: tint(color.ready, 0.4),
	pending: color.track,
};

const STATE_LABEL: Record<TodoState, string> = {
	done: "Done",
	in_progress: "In progress",
	pending: "Not started",
};

const Mark = ({ state }: { state: TodoState }) => {
	const { t } = useI18n();
	return (
		<View accessible accessibilityLabel={t(STATE_LABEL[state])} style={styles.mark}>
			{state === "done" ? (
				<CircleCheck size={14} strokeWidth={1.9} color={color.ready} />
			) : state === "in_progress" ? (
				<Spinner size={11} />
			) : (
				<View style={styles.ring} />
			)}
		</View>
	);
};

const TODO_TEXT = {
	done: { color: color.mutedForeground },
	in_progress: { color: color.foregroundStrong },
	pending: { color: color.secondaryForeground },
} as const;

/** The agent's to-do list. */
export const TodosCard = ({ item }: { item: ItemOf<"todos"> }) => {
	const { t } = useI18n();

	const done = item.todos.filter((todo) => todo.state === "done").length;
	return (
		<View accessibilityLabel={t("Plan")} style={[styles.card, styles.todos]}>
			<View style={[styles.head, styles.todosHead]}>
				<ListChecks {...ICON} />
				<Text style={styles.title}>{t("Plan")}</Text>
				<Text style={styles.count}>
					{t("{{done}} of {{total}} done", {
						done: formatNumber(done),
						total: formatNumber(item.todos.length),
					})}
				</Text>
				<View style={styles.bars}>
					{item.todos.map((todo, i) => (
						// The list is replaced whole on each change, so a step's place is its identity.
						<View key={i} style={[styles.bar, { backgroundColor: BAR[todo.state] }]} />
					))}
				</View>
			</View>
			{item.todos.map((todo, i) => (
				<View key={i} style={styles.todo}>
					<Mark state={todo.state} />
					<Text selectable style={[styles.todoText, TODO_TEXT[todo.state]]}>
						{todo.text}
					</Text>
				</View>
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	card: {
		paddingHorizontal: 12,
		paddingTop: 10,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	plan: { gap: 8, paddingBottom: 12 },
	todos: { gap: 4, paddingBottom: 8 },
	head: { flexDirection: "row", alignItems: "center", gap: 8 },
	todosHead: { paddingBottom: 4 },
	title: { color: color.foreground, fontSize: 12.5, fontWeight: "600" },
	count: { color: color.mutedForeground, fontSize: 12.5, fontVariant: ["tabular-nums"] },
	writing: {
		marginLeft: "auto",
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
	},
	writingText: { color: color.mutedForeground, fontSize: size.tiny },
	// A long list's bars shrink rather than push past the card.
	bars: { flex: 1, flexDirection: "row", justifyContent: "flex-end", gap: 3 },
	bar: { flexBasis: 22, flexShrink: 1, minWidth: 3, height: 4, borderRadius: 2 },
	todo: { minHeight: 22, flexDirection: "row", alignItems: "center", gap: 9 },
	mark: { width: 14, height: 14, alignItems: "center", justifyContent: "center" },
	ring: {
		width: 11,
		height: 11,
		borderRadius: 6,
		borderWidth: 1.5,
		borderColor: color.subtleForeground,
	},
	todoText: { flexShrink: 1, minWidth: 0, fontSize: 12.5, lineHeight: 18 },
});
