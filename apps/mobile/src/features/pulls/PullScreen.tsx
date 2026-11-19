// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One PR, as the desktop's viewer shows it: its number and title, its
// state, author and branches, a link to it on GitHub, and four tabs, its
// Summary with Merge and Approve, its changed Files with their diffs, its
// Checks and its Conversation. Opened from a row on the Pulls screen or a
// PR chip on an agent's screen.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import { readOf } from "@maxhq/shared/pulls/read";
import { pullUrl } from "@maxhq/shared/pulls/urls";

import type { ScreenProps } from "../../navigation";
import { useServices } from "../../services";
import { color, font, size } from "../../theme";
import { Header } from "../../ui/Header";
import { usePullToRefresh } from "../../ui/Refresh";
import { OutLink, PullStateBadge } from "./look";
import { pullId } from "./pulls";
import { PullSummaryTab } from "./PullSummary";
import { PullChecksTab, PullConversationTab, PullFilesTab } from "./PullTabs";
import { usePullDrops, useRead } from "./read";
import { Tabs } from "./Tabs";

type PullTab = "summary" | "files" | "checks" | "conversation";

export const PullScreen = ({ navigation, route }: ScreenProps<"Pull">) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { device, repo, number } = route.params;
	const key = useMemo(() => ({ repo, number }), [repo, number]);
	const [tab, setTab] = useState<PullTab>("summary");
	const pr = useRead(
		`${device}\n${pullId(key)}`,
		async () => readOf(await source.getPull(device, repo, number), "pull").pull,
		usePullDrops()
	);
	const refresh = usePullToRefresh(pr.reload);
	const pull = pr.data?.summary.pull;
	const shown = formatNumber(number, { useGrouping: false });

	return (
		<View style={styles.screen}>
			<Header
				title={t("Pull request #{{number}}", { number: shown })}
				subtitle={
					<Text style={styles.sub} numberOfLines={1}>
						{repo}
					</Text>
				}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			<View style={styles.top}>
				{pull ? (
					<Text style={styles.title} numberOfLines={3} accessibilityRole="header">
						{pull.title}
					</Text>
				) : null}
				<View style={styles.meta}>
					{pull ? (
						<>
							<PullStateBadge pull={pull} />
							<Text style={styles.author} numberOfLines={1}>
								{pull.author}
							</Text>
						</>
					) : null}
					<View style={styles.out}>
						<OutLink url={pullUrl(key)}>{t("Open on GitHub")}</OutLink>
					</View>
				</View>
				{pull ? (
					<Text style={styles.branches} numberOfLines={1} ellipsizeMode="middle">
						{t("{{head}} into {{base}}", { head: pull.head, base: pull.base })}
					</Text>
				) : null}
				<Tabs
					tabs={[
						{ value: "summary", label: t("Summary") },
						{ value: "files", label: t("Files") },
						{ value: "checks", label: t("Checks") },
						{ value: "conversation", label: t("Conversation") },
					]}
					value={tab}
					onChange={setTab}
				/>
			</View>
			<View style={styles.body}>
				{tab === "summary" ? (
					pr.data ? (
						<PullSummaryTab key={pullId(key)} pr={pr.data} refresh={refresh} />
					) : (
						<View style={styles.pad}>
							{pr.error ? (
								<Text
									style={[styles.quiet, styles.error]}
									accessibilityRole="alert"
								>
									{t("Couldn't read the pull request. {{message}}", {
										message: pr.error,
									})}
								</Text>
							) : (
								<Text style={styles.quiet}>{t("Reading the pull request")}</Text>
							)}
						</View>
					)
				) : tab === "files" ? (
					<PullFilesTab device={device} pull={key} />
				) : tab === "checks" ? (
					<PullChecksTab device={device} pull={key} />
				) : (
					<PullConversationTab device={device} pull={key} />
				)}
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	sub: { color: color.mutedForeground, fontSize: size.meta, fontFamily: font.mono },
	top: {
		gap: 8,
		paddingHorizontal: 14,
		paddingTop: 12,
		paddingBottom: 10,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
	},
	title: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "600" },
	meta: { flexDirection: "row", alignItems: "center", gap: 8 },
	author: { flexShrink: 1, color: color.mutedForeground, fontSize: size.meta },
	out: { marginLeft: "auto" },
	branches: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.tiny },
	body: { flex: 1 },
	pad: { padding: 14 },
	quiet: { color: color.mutedForeground, fontSize: size.small },
	error: { color: color.errorText },
});
