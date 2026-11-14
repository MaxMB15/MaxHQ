// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent's thinking, folded into one line that says how long it took.

import { useI18n } from "@maxhq/shared/i18n";

import { Pressable, StyleSheet, Text, View } from "react-native";

import { useFold } from "@maxhq/shared/agent-view/find-context";
import { isOpen, tookOf } from "@maxhq/shared/agent-view/format";
import type { TimelineItem } from "@maxhq/shared/bindings";
import { formatDuration } from "@maxhq/shared/status/format";

import { color, radius, size } from "../../theme";
import { Spinner } from "../../ui/Dot";
import { Elapsed } from "../../ui/Elapsed";
import { ChevronDown, ChevronRight, Lightbulb } from "../../ui/glyphs";
import { withCaret } from "../../ui/markdown";
import { MarkdownText } from "../../ui/MarkdownText";
import { useFeed } from "./feed-context";

/** A reasoning item. */
export const Reasoning = ({ item }: { item: Extract<TimelineItem, { kind: "reasoning" }> }) => {
	const { t } = useI18n();

	const { vendor } = useFeed();
	const [open, setOpen] = useFold(`open:${item.id}`, item.id);
	const live = item.streaming || isOpen(item.status);
	const took = tookOf(item);
	const hasText = item.text.trim() !== "";
	const label = live
		? t("Thinking")
		: took !== null
			? t("Thought for {{duration}}", { duration: formatDuration(took) })
			: t("Thought");
	const Chevron = open ? ChevronDown : ChevronRight;
	const line = (
		<>
			<View style={styles.icon}>
				{live ? (
					<Spinner size={12} />
				) : (
					<Lightbulb size={14} strokeWidth={1.9} color={color.mutedForeground} />
				)}
			</View>
			<Text style={styles.label}>
				{label}
				{live && item.started_at !== null ? (
					<>
						{" "}
						<Elapsed since={item.started_at} />
					</>
				) : null}
			</Text>
			{hasText ? <Chevron size={10} strokeWidth={2.4} color={color.mutedForeground} /> : null}
		</>
	);
	return (
		<View style={styles.step}>
			{hasText ? (
				<Pressable
					accessibilityRole="button"
					accessibilityState={{ expanded: open }}
					onPress={() => setOpen(!open)}
					style={({ pressed }) => [styles.row, pressed ? styles.pressed : null]}
				>
					{line}
				</Pressable>
			) : (
				<View style={styles.row}>{line}</View>
			)}
			{open && hasText ? (
				<View style={styles.body}>
					<MarkdownText
						text={item.streaming ? withCaret(item.text) : item.text}
						quiet={color.mutedForeground}
					/>
					{vendor === "codex" ? (
						<Text style={styles.note}>
							{t("Summary from Codex. The full reasoning isn't shared.")}
						</Text>
					) : null}
				</View>
			) : null}
		</View>
	);
};

const styles = StyleSheet.create({
	step: { gap: 6 },
	row: {
		minHeight: 30,
		alignSelf: "flex-start",
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingLeft: 6,
		paddingRight: 8,
		borderRadius: radius.md,
	},
	pressed: { backgroundColor: color.accent },
	icon: { width: 14, height: 14, alignItems: "center", justifyContent: "center" },
	label: { color: color.mutedForeground, fontSize: 12.5, fontVariant: ["tabular-nums"] },
	body: {
		marginLeft: 13,
		gap: 8,
		paddingLeft: 16,
		borderLeftWidth: 2,
		borderLeftColor: color.track,
	},
	note: { color: color.subtleForeground, fontSize: size.pill },
});
