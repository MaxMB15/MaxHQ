// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Plan usage, as the desktop's usage popover shows it: each Claude and
// Codex login with its windows, how much of each is used and when it
// resets, and Cursor's month once a Mac turns Cursor usage on. Below them,
// a cloud model server with credits, such as OpenRouter, shows what's
// left. Pulling the list down checks usage now.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import type { Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { type CreditsView, serverCreditRows } from "@maxhq/shared/local-models/credits";
import { formatAgo, formatPercent } from "@maxhq/shared/status/format";
import { resetCreditsLine } from "@maxhq/shared/status/reset-credits";
import {
	accountName,
	accountWindows,
	atLimit,
	blockingWindow,
	cycleWindows,
	type WindowView,
} from "@maxhq/shared/status/usage";
import type { Subscription } from "@maxhq/shared/status/subscriptions";
import {
	accountStatus,
	planNote,
	resetText,
	usageCheckFailure,
	usagePlans,
	weekNote,
} from "@maxhq/shared/status/usage-text";
import type { ViewState } from "@maxhq/shared/view/replica";
import { newestUsageCheck } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size, tint } from "../../theme";
import { Card } from "../../ui/Card";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Header } from "../../ui/Header";
import { usePullToRefresh } from "../../ui/Refresh";
import { Bar } from "./Bar";

export const UsageScreen = ({ navigation }: ScreenProps<"Usage">) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(10_000);
	const list = usagePlans(view);
	const credited = serverCreditRows(view, now);
	const checked = newestUsageCheck(view);
	const [failure, setFailure] = useState<string | null>(null);
	const refreshControl = usePullToRefresh(() => {
		setFailure(null);
		return source.refreshUsage(null).then(
			(result) => setFailure(usageCheckFailure(result)),
			() => setFailure(t("Couldn't check usage."))
		);
	});
	return (
		<View style={styles.screen}>
			<Header
				title={t("Usage")}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			<ConnectionBanner />
			<ScrollView contentContainerStyle={styles.content} refreshControl={refreshControl}>
				<Text style={styles.summary}>
					{checked !== undefined
						? t("Updated {{time}}. Pull down to check now.", {
								time: formatAgo(checked, now),
							})
						: t("Not checked yet. Pull down to check now.")}
				</Text>
				{failure ? (
					<Text accessibilityRole="alert" style={styles.failure}>
						{failure}
					</Text>
				) : null}
				{list.length === 0 ? (
					<Text style={styles.summary}>
						{t("No Claude or Codex login found on your devices yet.")}
					</Text>
				) : null}
				{list.length > 0 || credited.length > 0 ? (
					<Card label={t("Plan usage")}>
						{list.map((plan, i) => (
							<AccountSection
								key={plan.account.id}
								plan={plan}
								view={view}
								now={now}
								first={i === 0}
							/>
						))}
						{credited.map(({ server, view: credits }, i) => (
							<CreditsSection
								key={server.id}
								name={server.label}
								credits={credits}
								first={list.length === 0 && i === 0}
							/>
						))}
					</Card>
				) : null}
			</ScrollView>
		</View>
	);
};

const AccountSection = ({
	plan,
	view,
	now,
	first,
}: {
	plan: Subscription;
	view: ViewState;
	now: Timestamp;
	first: boolean;
}) => {
	const { account } = plan;
	const windows = [...accountWindows(account), ...cycleWindows(account)];
	const limited = atLimit(account);
	const blocking = blockingWindow(account)?.label;
	const failureNote = planNote(view, plan, now);
	const note = weekNote(windows, now);
	// Only the desktop app spends a reset, so the phone shows the count alone.
	const banked = resetCreditsLine(account);
	return (
		<View style={[styles.account, !first && styles.line, limited && styles.limited]}>
			<View style={styles.head}>
				<Text style={styles.name} numberOfLines={1}>
					{accountName(account)}
				</Text>
				<Text style={[styles.status, limited && styles.busyText]} numberOfLines={1}>
					{accountStatus(view, plan)}
				</Text>
			</View>
			{windows.map((w) => (
				<WindowRow key={w.label} window={w} now={now} blocking={blocking === w.label} />
			))}
			{note ? <Text style={[styles.note, styles.busyText]}>{note}</Text> : null}
			{banked ? <Text style={styles.note}>{banked}</Text> : null}
			{failureNote ? <Text style={styles.note}>{failureNote}</Text> : null}
		</View>
	);
};

/**
 * A cloud model server's credits, named after the server. After a failed
 * read the last figures show dimmed, with why under them.
 */
const CreditsSection = ({
	name,
	credits,
	first,
}: {
	name: string;
	credits: CreditsView;
	first: boolean;
}) => {
	useI18n();
	return (
		<View style={[styles.account, !first && styles.line]}>
			<View style={[styles.credits, credits.stale && styles.stale]}>
				<View style={styles.head}>
					<Text style={styles.name} numberOfLines={1}>
						{name}
					</Text>
					<Text style={styles.status} numberOfLines={1}>
						{credits.status}
					</Text>
				</View>
				{credits.bars.map(({ window: w, text }) => (
					<View
						key={w.label}
						style={styles.window}
						accessible
						accessibilityLabel={[w.label, text].join(", ")}
					>
						<Text style={styles.windowLabel} numberOfLines={1}>
							{w.label}
						</Text>
						{w.percent === null ? (
							<View style={styles.bar} />
						) : (
							<Bar percent={w.percent} full={w.usedUp} style={styles.bar} />
						)}
						<Text style={[styles.percent, w.usedUp && styles.usedUp]}>
							{w.percent === null ? "" : formatPercent(w.percent)}
						</Text>
						<Text style={[styles.reset, styles.creditsText]} numberOfLines={1}>
							{text}
						</Text>
					</View>
				))}
			</View>
			{credits.stale ? <Text style={styles.note}>{credits.title}</Text> : null}
		</View>
	);
};

const WindowRow = ({
	window,
	now,
	blocking,
}: {
	window: WindowView;
	now: Timestamp;
	blocking: boolean;
}) => {
	const { t } = useI18n();

	const percent = window.percent === null ? "" : formatPercent(window.percent);
	const reset = resetText(window, now);
	return (
		<View
			style={styles.window}
			accessible
			accessibilityLabel={[window.label, percent && t("{{percent}} used", { percent }), reset]
				.filter(Boolean)
				.join(", ")}
		>
			<Text style={styles.windowLabel} numberOfLines={1}>
				{window.label}
			</Text>
			<Bar percent={window.percent} full={window.usedUp} style={styles.bar} />
			<Text style={[styles.percent, window.usedUp && styles.usedUp]}>{percent}</Text>
			<Text style={[styles.reset, blocking && styles.busyText]} numberOfLines={1}>
				{reset}
			</Text>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 12 },
	summary: { color: color.mutedForeground, fontSize: size.meta, paddingHorizontal: 2 },
	failure: { color: color.errorText, fontSize: size.meta, paddingHorizontal: 2 },
	account: { paddingHorizontal: 14, paddingVertical: 12, gap: 8 },
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	limited: { backgroundColor: tint(color.busy, 0.05) },
	head: { flexDirection: "row", alignItems: "baseline", justifyContent: "space-between", gap: 8 },
	name: { flexShrink: 1, color: color.foreground, fontSize: size.small, fontWeight: "600" },
	status: { color: color.mutedForeground, fontSize: size.tiny },
	busyText: { color: color.busyText },
	window: { flexDirection: "row", alignItems: "center", gap: 8 },
	windowLabel: { width: 64, color: color.secondaryForeground, fontSize: size.tiny },
	bar: { flex: 1, height: 5, borderRadius: 3 },
	percent: {
		width: 36,
		textAlign: "right",
		color: color.secondaryForeground,
		fontSize: size.tiny,
		fontVariant: ["tabular-nums"],
	},
	usedUp: { color: color.busy },
	reset: { width: 108, color: color.mutedForeground, fontSize: size.tiny },
	note: { color: color.mutedForeground, fontSize: size.pill },
	credits: { gap: 8 },
	stale: { opacity: 0.55 },
	creditsText: { width: 128 },
});
