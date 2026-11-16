// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A sub-agent that finished: its feed, read once from its lead's session,
// with find, and a note in place of the composer that points to its lead.

import { useI18n } from "@maxhq/shared/i18n";

import { useCallback, useRef } from "react";
import { FlatList, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useStore } from "zustand";

import { FindProvider } from "@maxhq/shared/agent-view/find-context";
import { finishedTitle } from "@maxhq/shared/agent-view/finished";
import type { Agent, AgentId } from "@maxhq/shared/bindings";
import type { FeedRow } from "@maxhq/shared/timeline/output";
import { useTimeline } from "@maxhq/shared/timeline/use-timeline";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Header } from "../../ui/Header";
import { FeedProvider, useFeedRows } from "./feed-context";
import { FindBar, FindButton, useFeedFind } from "./FeedFind";
import { FeedItem } from "./FeedRow";
import { SubAgentFinished } from "./OutsideBanner";

/** The history of `id`, a sub-agent that finished, which `lead`'s session holds. */
export const FinishedAgent = ({
	id,
	lead,
	back,
}: {
	id: AgentId;
	lead: Agent;
	back: { label: string; onPress: () => void };
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const device = useView((s) => {
		const workspace = s.view.tables.workspace.get(lead.workspace);
		return workspace ? deviceName(s.view, workspace.device) : t("this device");
	});
	const timeline = useTimeline(source, id);
	const status = useStore(timeline.store, (s) => s.status);
	const error = useStore(timeline.store, (s) => s.error);
	const hasOlder = useStore(timeline.store, (s) => s.hasOlder);
	const loadingOlder = useStore(timeline.store, (s) => s.loadingOlder);
	const rows = useFeedRows(timeline.store);
	const insets = useSafeAreaInsets();
	const list = useRef<FlatList<FeedRow>>(null);
	const atEnd = useRef(true);
	const found = useFeedFind(id, timeline, rows, list, atEnd);
	const { goal } = found;
	const renderItem = useCallback(
		({ item }: { item: FeedRow }) => <FeedItem id={item.id} gap={item.gap} goal={goal} />,
		[goal]
	);
	const empty = status !== "loading" && !error && rows.length === 0;

	return (
		<View style={styles.screen}>
			<Header
				title={finishedTitle(id) ?? t("Sub-agent")}
				back={back}
				right={<FindButton find={found.find} />}
				subtitle={
					<Text style={styles.place} numberOfLines={1}>
						{t("Finished, read only")}
					</Text>
				}
			/>
			<FindBar find={found.find} />
			<ConnectionBanner />
			<FindProvider store={found.find.store}>
				<FeedProvider
					agent={{ ...lead, id }}
					device={device}
					store={timeline.store}
					readOnly
				>
					<FlatList
						ref={list}
						data={rows}
						keyExtractor={(row) => row.id}
						contentContainerStyle={styles.content}
						renderItem={renderItem}
						initialNumToRender={24}
						onViewableItemsChanged={found.onViewableItemsChanged}
						ListHeaderComponent={
							<View style={styles.head}>
								{hasOlder ? (
									<Pressable
										accessibilityRole="button"
										disabled={loadingOlder}
										onPress={timeline.loadOlder}
										style={styles.olderButton}
									>
										<Text style={styles.older}>
											{loadingOlder
												? t("Loading")
												: t("Load earlier activity")}
										</Text>
									</Pressable>
								) : null}
								{status === "loading" ? (
									<Text style={styles.note}>{t("Loading")}</Text>
								) : null}
								{error ? <Text style={styles.failed}>{error}</Text> : null}
								{empty ? (
									<Text style={styles.note}>
										{t("MaxHQ found no history for this sub-agent.")}
									</Text>
								) : null}
							</View>
						}
						onScroll={({ nativeEvent: e }) => {
							atEnd.current =
								e.contentOffset.y + e.layoutMeasurement.height >=
								e.contentSize.height - 80;
						}}
						scrollEventThrottle={100}
						onContentSizeChange={() => {
							if (atEnd.current) list.current?.scrollToEnd({ animated: false });
						}}
					/>
				</FeedProvider>
			</FindProvider>
			<View style={{ paddingBottom: insets.bottom }}>
				<SubAgentFinished lead={lead} />
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { paddingHorizontal: 14, paddingVertical: 14 },
	place: { color: color.mutedForeground, fontSize: size.meta, marginTop: 1 },
	head: { gap: 10 },
	olderButton: { minHeight: 44, justifyContent: "center" },
	older: { color: color.primary, fontSize: size.small },
	note: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText, fontSize: size.meta },
});
