// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Your prompts, from the More sheet: the prompts you sent the agent, newest
// first under Latest message, with the one nearest the top of the feed
// picked. A tap closes the sheet and scrolls the feed to that prompt. While
// older pages wait, the list ends in Load earlier activity, which brings
// the prompts in them.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { memo, useMemo } from "react";
import { FlatList, Modal, Pressable, StyleSheet, Text, View } from "react-native";
import { useStore } from "zustand";

import type { ItemId } from "@maxhq/shared/bindings";
import type { FeedRow } from "@maxhq/shared/timeline/output";
import type { Timeline } from "@maxhq/shared/timeline/store";

import { color, radius, size, TOUCH } from "../../theme";
import { Header } from "../../ui/Header";
import { type PromptEntry, promptAtTop, promptEntries } from "./prompts";

const Entry = memo(
	({
		entry,
		picked,
		onPick,
	}: {
		entry: PromptEntry;
		picked: boolean;
		onPick: (row: number) => void;
	}) => (
		<Pressable
			accessibilityRole="button"
			accessibilityState={{ selected: picked }}
			onPress={() => onPick(entry.row)}
			style={({ pressed }) => [
				styles.entry,
				picked && styles.picked,
				pressed && styles.pressed,
			]}
		>
			<Text style={styles.number}>
				{entry.number === null ? "" : formatNumber(entry.number)}
			</Text>
			<Text style={styles.line} numberOfLines={2}>
				{entry.line || "…"}
			</Text>
		</Pressable>
	)
);
Entry.displayName = "PromptEntry";

/**
 * The sheet over `timeline`'s feed, open while `top` isn't null: the row at
 * the top of the screen as it opened, or undefined before the feed said.
 * `onJump` scrolls the feed to a row, or to its end.
 */
export const PromptsSheet = ({
	timeline,
	rows,
	top,
	onClose,
	onJump,
}: {
	timeline: Timeline;
	rows: readonly FeedRow[];
	top: { readonly row: string | undefined } | null;
	onClose: () => void;
	onJump: (row: number | "end") => void;
}) => {
	const { t } = useI18n();

	const items = useStore(timeline.store, (s) => s.items);
	const hasOlder = useStore(timeline.store, (s) => s.hasOlder);
	const loadingOlder = useStore(timeline.store, (s) => s.loadingOlder);
	const entries = useMemo(() => promptEntries(rows, items, hasOlder), [rows, items, hasOlder]);
	const at: ItemId | null = top ? promptAtTop(entries, rows, top.row) : null;
	const jump = (row: number | "end") => {
		onClose();
		onJump(row);
	};

	return (
		<Modal
			visible={top !== null}
			animationType="slide"
			presentationStyle="pageSheet"
			onRequestClose={onClose}
		>
			<View style={styles.sheet}>
				<Header
					title={t("Your prompts")}
					sheet
					back={{ label: t("Close"), onPress: onClose, close: true }}
				/>
				<FlatList
					data={entries}
					keyExtractor={(entry) => entry.id}
					contentContainerStyle={styles.list}
					ListHeaderComponent={
						<Pressable
							accessibilityRole="button"
							onPress={() => jump("end")}
							style={({ pressed }) => [styles.entry, pressed && styles.pressed]}
						>
							<Text style={styles.latest}>{t("Latest message")}</Text>
						</Pressable>
					}
					renderItem={({ item }) => (
						<Entry entry={item} picked={item.id === at} onPick={jump} />
					)}
					ListEmptyComponent={
						hasOlder ? undefined : (
							<Text style={styles.note}>{t("No prompts yet")}</Text>
						)
					}
					ListFooterComponent={
						hasOlder ? (
							<Pressable
								accessibilityRole="button"
								disabled={loadingOlder}
								onPress={timeline.loadOlder}
								style={styles.older}
							>
								<Text style={styles.olderText}>
									{loadingOlder ? t("Loading") : t("Load earlier activity")}
								</Text>
							</Pressable>
						) : undefined
					}
				/>
			</View>
		</Modal>
	);
};

const styles = StyleSheet.create({
	sheet: { flex: 1, backgroundColor: color.background },
	list: { padding: 12, paddingBottom: 40 },
	entry: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		borderRadius: radius.md,
		paddingHorizontal: 12,
		paddingVertical: 10,
	},
	picked: { backgroundColor: color.secondary },
	pressed: { backgroundColor: color.accent },
	number: {
		minWidth: 22,
		color: color.mutedForeground,
		fontSize: size.meta,
		fontVariant: ["tabular-nums"],
		textAlign: "right",
	},
	line: { flex: 1, color: color.foreground, fontSize: size.body },
	latest: { color: color.primary, fontSize: size.body },
	note: { color: color.mutedForeground, fontSize: size.meta, padding: 12 },
	older: { minHeight: TOUCH, justifyContent: "center", paddingHorizontal: 12 },
	olderText: { color: color.primary, fontSize: size.small },
});
