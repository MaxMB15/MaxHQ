// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A task's runs and the requests it took, on two tabs, as the desktop's
// dialog shows them. Each run names the agent it reached, which opens it.
// A timed task takes no requests, so it opens on its runs. Both are read
// from the task's device when the screen opens, and again on a pull.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import type { AgentId, Run, TaskDelivery } from "@maxhq/shared/bindings";
import type { DeliveriesResult, RunsResult } from "@maxhq/shared/data/source";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatWhen } from "@maxhq/shared/status/format";
import { takesRequests } from "@maxhq/shared/tasks/draft";
import {
	deliveryFacts,
	deliveryLabel,
	deliveryTone,
	RUN_CAUSE,
	RUN_OUTCOME,
	runTone,
} from "@maxhq/shared/tasks/format";
import { taskById } from "@maxhq/shared/tasks/place";
import { errorText } from "@maxhq/shared/timeline/store";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, radius, size } from "../../theme";
import { Header } from "../../ui/Header";
import { usePullToRefresh } from "../../ui/Refresh";
import { Badge } from "../pulls/look";
import { type Reading, useRead } from "../pulls/read";
import { Tabs } from "../pulls/Tabs";

type Side = "requests" | "runs";

/** What came of a read of runs or requests, or the failure as an error. */
const listed = async <T,>(result: Promise<RunsResult | DeliveriesResult>): Promise<T[]> => {
	const answer = await result;
	if (answer.status === "error") throw new Error(errorText(answer.error));
	return answer.data as T[];
};

const Empty = ({ children, error }: { children: string; error?: boolean }) => (
	<Text
		style={[styles.empty, error && styles.failed]}
		accessibilityRole={error ? "alert" : undefined}
	>
		{children}
	</Text>
);

const RunRow = ({
	run,
	first,
	now,
	agent,
	onAgent,
}: {
	run: Run;
	first: boolean;
	now: number;
	/** The title of the agent it reached, or null once that agent is gone. */
	agent: string | null;
	onAgent: (agent: AgentId) => void;
}) => {
	const { t } = useI18n();
	return (
		<View style={[styles.row, !first && styles.divider]}>
			<Badge tone={runTone(run.outcome)}>{RUN_OUTCOME[run.outcome]}</Badge>
			<View style={styles.text}>
				<Text style={styles.line}>
					{RUN_CAUSE[run.cause]}, {formatWhen(run.started, now)}
				</Text>
				{run.error ? <Text style={styles.error}>{run.error}</Text> : null}
			</View>
			{run.agent ? (
				agent ? (
					<Pressable
						accessibilityRole="link"
						accessibilityLabel={agent}
						hitSlop={8}
						onPress={() => run.agent && onAgent(run.agent)}
						style={({ pressed }) => [styles.agent, pressed && styles.pressed]}
					>
						<Text style={styles.agentText} numberOfLines={1}>
							{agent}
						</Text>
					</Pressable>
				) : (
					<Text style={styles.gone}>{t("Agent gone")}</Text>
				)
			) : null}
		</View>
	);
};

const DeliveryRow = ({
	delivery,
	first,
	now,
}: {
	delivery: TaskDelivery;
	first: boolean;
	now: number;
}) => {
	const { t } = useI18n();
	const facts = deliveryFacts(delivery);
	return (
		<View style={[styles.row, !first && styles.divider]}>
			<Badge tone={deliveryTone(delivery.outcome)}>{deliveryLabel(delivery.outcome)}</Badge>
			<View style={styles.text}>
				<Text style={styles.line}>
					{formatWhen(delivery.received, now)}
					<Text style={styles.muted}>, {facts.join(", ")}</Text>
				</Text>
				{delivery.missing_fields.length > 0 ? (
					<Text style={styles.muted}>
						{t("Missing {{value2}}", { value2: delivery.missing_fields.join(", ") })}
					</Text>
				) : null}
				{delivery.refused_fields?.length ? (
					<Text style={styles.muted}>
						{t("Left out free text {{value2}}", {
							value2: delivery.refused_fields.join(", "),
						})}
					</Text>
				) : null}
				{delivery.error ? <Text style={styles.error}>{delivery.error}</Text> : null}
			</View>
		</View>
	);
};

const Runs = ({
	reading,
	now,
	onAgent,
}: {
	reading: Reading<Run[]>;
	now: number;
	onAgent: (agent: AgentId) => void;
}) => {
	const { t } = useI18n();
	const agents = useView((s) => s.view.tables.agent);
	const runs = reading.data;
	if (!runs)
		return reading.error ? (
			<Empty error>{t("Couldn't read the runs")}</Empty>
		) : (
			<Empty>{t("Reading the runs")}</Empty>
		);
	if (runs.length === 0) return <Empty>{t("No runs yet")}</Empty>;
	return (
		<View style={styles.list} accessibilityLabel={t("Runs")}>
			{runs.map((run, i) => (
				<RunRow
					key={`${run.started}:${run.cause}`}
					run={run}
					first={i === 0}
					now={now}
					agent={run.agent ? (agents.get(run.agent)?.title ?? null) : null}
					onAgent={onAgent}
				/>
			))}
		</View>
	);
};

const Deliveries = ({ reading, now }: { reading: Reading<TaskDelivery[]>; now: number }) => {
	const { t } = useI18n();
	const deliveries = reading.data;
	if (!deliveries)
		return reading.error ? (
			<Empty error>{t("Couldn't read the requests")}</Empty>
		) : (
			<Empty>{t("Reading the requests")}</Empty>
		);
	if (deliveries.length === 0) return <Empty>{t("No requests yet")}</Empty>;
	return (
		<View style={styles.list} accessibilityLabel={t("Requests")}>
			{deliveries.map((delivery, i) => (
				<DeliveryRow key={delivery.id} delivery={delivery} first={i === 0} now={now} />
			))}
		</View>
	);
};

export const TaskActivityScreen = ({ navigation, route }: ScreenProps<"TaskActivity">) => {
	const { t } = useI18n();

	const { source } = useServices();
	const id = route.params.task;
	const view = useView((s) => s.view);
	const found = taskById(view, id);
	const now = useNow(60_000);
	const [side, setSide] = useState<Side>(() =>
		found && takesRequests(found.task.trigger.type) ? "requests" : "runs"
	);
	const runs = useRead(id, () => listed<Run>(source.listRuns(id)));
	const deliveries = useRead(id, () => listed<TaskDelivery>(source.listDeliveries(id)));
	const refresh = usePullToRefresh(() => Promise.all([runs.reload(), deliveries.reload()]));
	const back = { label: t("Back"), onPress: () => navigation.goBack() };

	if (!found)
		return (
			<View style={styles.screen}>
				<Header title={t("History")} back={back} />
				<Empty>{t("This task is gone.")}</Empty>
			</View>
		);

	return (
		<View style={styles.screen}>
			<Header title={found.task.name} subtitle={t("History")} back={back} />
			<View style={styles.tabs}>
				<Tabs
					tabs={[
						{ value: "requests", label: t("Deliveries") },
						{ value: "runs", label: t("Runs") },
					]}
					value={side}
					onChange={setSide}
				/>
			</View>
			<ScrollView contentContainerStyle={styles.content} refreshControl={refresh}>
				{side === "requests" ? (
					<Deliveries reading={deliveries} now={now} />
				) : (
					<Runs
						reading={runs}
						now={now}
						onAgent={(agent) => navigation.navigate("Agent", { agent })}
					/>
				)}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	tabs: { paddingHorizontal: 14, paddingTop: 10 },
	content: { padding: 14, paddingBottom: 40 },
	list: {
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.card,
		overflow: "hidden",
	},
	row: {
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 14,
		paddingVertical: 10,
		minHeight: 44,
	},
	divider: { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: color.border },
	text: { flex: 1, minWidth: 0, gap: 2 },
	line: { color: color.foreground, fontSize: size.small },
	muted: { color: color.mutedForeground, fontSize: size.meta },
	error: { color: color.errorText, fontSize: size.meta },
	agent: {
		maxWidth: "40%",
		minHeight: 32,
		justifyContent: "center",
		paddingHorizontal: 10,
		borderRadius: radius.sm,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	agentText: { color: color.foreground, fontSize: size.meta, fontWeight: "600" },
	gone: { color: color.mutedForeground, fontSize: size.meta },
	empty: {
		paddingVertical: 24,
		textAlign: "center",
		color: color.mutedForeground,
		fontSize: size.small,
	},
	failed: { color: color.errorText },
});
