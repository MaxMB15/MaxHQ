// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's workflow runs, live ones first, as the desktop's Workflows
// entry: each run's state, its name and workflow, and its progress while
// live or how long ago it ended. A tap opens the run, and a long press its
// menu, with Stop run and Copy run ID.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useCallback, useMemo } from "react";
import { FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import type { Timestamp, WorkflowRun } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { projectRuns } from "@maxhq/shared/workflow/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { Header } from "../../ui/Header";
import { Badge } from "../test-hosts/parts";
import { NodeGlyph } from "./NodeGlyph";
import { openRunMenu } from "./RunMenu";
import { runLine } from "./runs";

const ROW = 40;

const RunRow = memo(
	({
		run,
		now,
		onOpen,
		onMore,
	}: {
		run: WorkflowRun;
		now: Timestamp;
		onOpen: (run: WorkflowRun) => void;
		onMore: (run: WorkflowRun) => void;
	}) => {
		const { t } = useI18n();
		const line = runLine(run, now);
		return (
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={line.spoken}
				accessibilityActions={[{ name: "more", label: t("More") }]}
				onAccessibilityAction={() => onMore(run)}
				onPress={() => onOpen(run)}
				onLongPress={() => onMore(run)}
				style={({ pressed }) => [styles.row, pressed && styles.pressed]}
			>
				<NodeGlyph status={line.status} decorative />
				<View style={styles.text}>
					<Text style={styles.title} numberOfLines={1}>
						{line.label}
					</Text>
					<Text style={styles.sub} numberOfLines={1}>
						{line.workflow}
					</Text>
				</View>
				{line.needsYou ? <Badge label={t("Needs you")} tone="warn" /> : null}
				<Text style={[styles.side, !run.done && styles.live]}>{line.side}</Text>
			</Pressable>
		);
	}
);
RunRow.displayName = "RunRow";

export const WorkflowsScreen = ({ navigation, route }: ScreenProps<"Workflows">) => {
	const { t } = useI18n();
	const services = useServices();
	const { project } = route.params;
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const runs = useMemo(() => projectRuns(view, project), [view, project]);
	const name = view.tables.project.get(project)?.name;
	const open = useCallback(
		(run: WorkflowRun) => navigation.navigate("Run", run.key),
		[navigation]
	);
	const more = useCallback(
		(run: WorkflowRun) => openRunMenu(services, run, () => open(run)),
		[services, open]
	);
	return (
		<View style={styles.screen}>
			<Header
				title={t("Workflows")}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
				subtitle={
					name === undefined ? undefined : (
						<Text style={styles.subtitle} numberOfLines={1}>
							{name}
						</Text>
					)
				}
			/>
			<FlatList
				data={runs}
				keyExtractor={(run) => run.id}
				renderItem={({ item }) => (
					<RunRow run={item} now={now} onOpen={open} onMore={more} />
				)}
				contentContainerStyle={styles.content}
				ListEmptyComponent={<Text style={styles.note}>{t("No runs yet")}</Text>}
			/>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	subtitle: { color: color.mutedForeground, fontSize: size.meta },
	content: { paddingHorizontal: 6, paddingTop: 4, paddingBottom: 32 },
	row: {
		minHeight: ROW,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 10,
		paddingVertical: 3,
		borderRadius: 6,
	},
	pressed: { backgroundColor: color.accent },
	text: { flex: 1, minWidth: 0, gap: 1 },
	title: { color: color.foregroundStrong, fontSize: size.body },
	sub: { color: color.mutedForeground, fontSize: size.meta },
	side: {
		color: color.mutedForeground,
		fontSize: size.meta,
		fontVariant: ["tabular-nums"],
	},
	live: { color: color.secondaryForeground },
	note: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
});
