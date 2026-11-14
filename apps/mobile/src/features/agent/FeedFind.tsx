// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Find on an agent's screen: the search button in its header, the bar
// that opens over the header with the keyboard up, and the mark on the
// row of the active match. Typing searches the whole timeline through
// maxd, Up and Down step through the matches, and the feed scrolls to each
// one's row. Cancel, the swipe back and Android's back close it and leave
// the feed where it is.

import { useI18n } from "@maxhq/shared/i18n";

import { usePreventRemove } from "@react-navigation/native";
import { type ReactNode, type RefObject, useCallback, useEffect, useMemo, useRef } from "react";
import { type FlatList, type ListViewToken, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useStore } from "zustand";
import type { StoreApi } from "zustand/vanilla";

import type { AgentId } from "@maxhq/shared/bindings";
import { MAX_FIND_QUERY } from "@maxhq/shared/find/search";
import { type Find, findLabel } from "@maxhq/shared/find/store";
import type { FeedRow } from "@maxhq/shared/timeline/output";
import type { Timeline } from "@maxhq/shared/timeline/store";

import { useServices } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { IconButton } from "../../ui/Header";
import { Chevron, Search } from "../../ui/Icon";
import { createFeedFind, type FeedGoal } from "./find";

/**
 * The find of an agent's feed, going to each match in `listRef`. Going
 * there stops the feed following new output, through `atEndRef`.
 */
export const useFeedFind = (
	agent: AgentId,
	timeline: Timeline,
	rows: readonly FeedRow[],
	listRef: RefObject<FlatList<FeedRow> | null>,
	atEndRef: RefObject<boolean>
) => {
	const { source } = useServices();
	const feed = useMemo(() => createFeedFind(source, agent, timeline), [source, agent, timeline]);
	useEffect(() => feed.follow(), [feed]);
	const { find } = feed;
	const open = useStore(find.store, (s) => s.open);
	const goal = useStore(feed.goal);
	// The swipe back and Android's back close find before they leave the agent.
	usePreventRemove(open, () => find.close());

	// The rows on screen, top first, as the list last said.
	const seen = useRef<{ top?: string; rows: ReadonlySet<string> }>({ rows: new Set() });
	const onViewableItemsChanged = useCallback(
		({ viewableItems }: { viewableItems: ListViewToken[] }) => {
			seen.current = {
				top: viewableItems[0]?.key,
				rows: new Set(viewableItems.map((token) => token.key)),
			};
		},
		[]
	);
	// Find starts from the top row on screen.
	useEffect(
		() =>
			find.readFrom(() => {
				const { top } = seen.current;
				return top === undefined ? undefined : { item: top, occurrence: 0 };
			}),
		[find]
	);

	// The goal the feed last went to, so each one moves it once.
	const went = useRef<FeedGoal | null>(null);
	useEffect(() => {
		if (goal === went.current || goal.row === null) return;
		const index = rows.findIndex((row) => row.id === goal.row);
		if (index < 0) return;
		// Another match in the row on screen leaves the feed where it is.
		const stays = went.current?.row === goal.row && seen.current.rows.has(goal.row);
		went.current = goal;
		if (stays) return;
		atEndRef.current = false;
		listRef.current?.scrollToIndex({ index, viewPosition: 0.3 });
	}, [goal, rows, listRef, atEndRef]);

	return { find, goal: feed.goal, onViewableItemsChanged };
};

/** The search button in an agent's header. */
export const FindButton = ({ find }: { find: Find }) => {
	const { t } = useI18n();
	return (
		<IconButton label={t("Find")} onPress={find.open}>
			<Search />
		</IconButton>
	);
};

/** The find bar, over the agent's header while find is open. */
export const FindBar = ({ find }: { find: Find }) => {
	const { t } = useI18n();

	const state = useStore(find.store);
	const insets = useSafeAreaInsets();
	if (!state.open) return null;

	const label = findLabel(state);
	const stuck = state.result === null || state.result.total === 0;
	return (
		<View style={[styles.bar, { paddingTop: insets.top }]}>
			<View style={styles.row}>
				<View style={styles.field}>
					<Search size={15} />
					<TextInput
						autoFocus
						value={state.typed}
						onChangeText={find.type}
						placeholder={t("Find in conversation")}
						placeholderTextColor={color.subtleForeground}
						accessibilityLabel={t("Find in conversation")}
						maxLength={MAX_FIND_QUERY}
						autoCapitalize="none"
						autoCorrect={false}
						spellCheck={false}
						returnKeyType="search"
						submitBehavior="submit"
						onSubmitEditing={() => find.step(1)}
						style={styles.input}
					/>
					{label.text ? (
						<Text
							accessibilityLiveRegion="polite"
							numberOfLines={1}
							style={[styles.count, label.none && styles.none]}
						>
							{label.text}
						</Text>
					) : null}
				</View>
				{state.status === "failed" ? (
					<Button label={t("Retry")} tone="link" small onPress={find.retry} />
				) : null}
				<IconButton
					label={t("Previous match")}
					disabled={stuck}
					onPress={() => find.step(-1)}
				>
					<Chevron dir="up" size={14} tint={color.secondaryForeground} />
				</IconButton>
				<IconButton label={t("Next match")} disabled={stuck} onPress={() => find.step(1)}>
					<Chevron dir="down" size={14} tint={color.secondaryForeground} />
				</IconButton>
				<Button label={t("Cancel")} tone="link" small onPress={find.close} />
			</View>
		</View>
	);
};

/** A feed row, marked while it holds the active match. */
export const FoundRow = ({
	goal,
	id,
	children,
}: {
	goal: StoreApi<FeedGoal>;
	id: string;
	children: ReactNode;
}) => {
	const marked = useStore(goal, (s) => s.row === id);
	return <View style={[styles.found, marked && styles.marked]}>{children}</View>;
};

// The mark reaches past the row, so the row keeps its place with or without it.
const REACH = { x: 8, y: 6 };

const styles = StyleSheet.create({
	// It covers the header, which is as tall.
	bar: {
		position: "absolute",
		top: 0,
		left: 0,
		right: 0,
		zIndex: 1,
		backgroundColor: color.background,
		borderBottomColor: color.border,
		borderBottomWidth: StyleSheet.hairlineWidth,
	},
	row: {
		height: 52,
		flexDirection: "row",
		alignItems: "center",
		gap: 2,
		paddingLeft: 12,
		paddingRight: 4,
	},
	field: {
		flex: 1,
		minWidth: 0,
		height: 36,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 10,
		borderRadius: radius.md,
		backgroundColor: color.secondary,
	},
	input: { flex: 1, minWidth: 0, color: color.foreground, fontSize: size.body, padding: 0 },
	count: {
		color: color.mutedForeground,
		fontSize: size.meta,
		fontVariant: ["tabular-nums"],
	},
	none: { color: color.destructive },
	found: {
		marginHorizontal: -REACH.x,
		marginVertical: -REACH.y,
		paddingHorizontal: REACH.x - StyleSheet.hairlineWidth,
		paddingVertical: REACH.y - StyleSheet.hairlineWidth,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: "transparent",
	},
	marked: {
		backgroundColor: tint(color.findActiveBg, 0.12),
		borderColor: tint(color.findActiveBg, 0.45),
	},
});
