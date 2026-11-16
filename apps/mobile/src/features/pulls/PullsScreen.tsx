// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's pull requests, as the desktop's Pulls page lists them: the
// open and closed ones on two tabs, sorted by merge readiness unless you
// pick another sort. Each row opens its PR, and each agent linked to it
// opens that agent. Opened from the agent list's menu, it shows the first
// project whose device's GitHub App reaches its repository, and the line
// under the title picks another. Pulling the list down reads it again.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { memo, useMemo, useState } from "react";
import { FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import type {
	AgentId,
	DeviceId,
	ProjectId,
	PullListState,
	PullSummary,
} from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { pullDevice, pullsDevice } from "@maxhq/shared/pulls/devices";
import { agentsWithPull } from "@maxhq/shared/pulls/links";
import { type PullSort, SORT_OPTIONS, sortPulls } from "@maxhq/shared/pulls/list";
import { readOf } from "@maxhq/shared/pulls/read";
import { formatAgoShort } from "@maxhq/shared/status/format";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, font, size } from "../../theme";
import { choose } from "../../ui/choose";
import { ArrowUpDown, ChevronDown } from "../../ui/glyphs";
import { Header } from "../../ui/Header";
import { usePullToRefresh } from "../../ui/Refresh";
import { Badge, ChecksMark, PullSize, ReviewMark, UnknownChecksBanner } from "./look";
import { pullId, pullProjects } from "./pulls";
import { usePullDrops, useRead } from "./read";
import { Tabs } from "./Tabs";

type RowProps = {
	row: PullSummary;
	device: DeviceId | undefined;
	agents: readonly { readonly id: AgentId; readonly title: string }[];
	ago: string;
	onOpen: (row: PullSummary, device: DeviceId) => void;
	onAgent: (agent: AgentId) => void;
};

const PullRow = memo(({ row, device, agents, ago, onOpen, onAgent }: RowProps) => {
	const { t } = useI18n();
	const { pull } = row;
	return (
		<View style={styles.item}>
			<Pressable
				accessibilityRole="link"
				disabled={!device}
				onPress={() => device && onOpen(row, device)}
				style={({ pressed }) => [styles.row, pressed && styles.pressed]}
			>
				<View style={styles.top}>
					<Text style={styles.number}>
						#{formatNumber(row.key.number, { useGrouping: false })}
					</Text>
					<Text style={styles.title} numberOfLines={2}>
						{pull.title}
					</Text>
					{pull.draft ? <Badge tone="plain">{t("Draft")}</Badge> : null}
				</View>
				<View style={styles.meta}>
					<Text style={styles.author} numberOfLines={1}>
						{pull.author}
					</Text>
					<ChecksMark state={pull.checks} />
					<ReviewMark state={pull.review} />
					<PullSize pull={pull} />
					<Text style={styles.ago} accessibilityLabel={`${t("Last changed")} ${ago}`}>
						{ago}
					</Text>
				</View>
			</Pressable>
			{agents.length > 0 ? (
				<View style={styles.agents}>
					{agents.map((agent) => (
						<Pressable
							key={agent.id}
							accessibilityRole="link"
							accessibilityHint={t("Open this agent")}
							hitSlop={{ top: 8, bottom: 8 }}
							onPress={() => onAgent(agent.id)}
							style={({ pressed }) => [styles.agent, pressed && styles.pressed]}
						>
							<Text style={styles.agentText} numberOfLines={1}>
								{agent.title}
							</Text>
						</Pressable>
					))}
				</View>
			) : null}
		</View>
	);
});
PullRow.displayName = "PullRow";

const PullList = ({
	project,
	state,
	sort,
	onOpen,
	onAgent,
}: {
	project: ProjectId;
	state: PullListState;
	sort: PullSort;
	onOpen: RowProps["onOpen"];
	onAgent: RowProps["onAgent"];
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const drops = usePullDrops();
	const list = useRead(
		`${project}\n${state}`,
		async () => readOf(await source.listPulls(project, state), "pulls").pulls,
		drops
	);
	const refresh = usePullToRefresh(list.reload);
	const rows = useMemo(() => (list.data ? sortPulls(list.data, sort) : []), [list.data, sort]);
	const unknown = rows.find((row) => row.pull.checks === "unknown");

	const empty = list.error
		? t("Couldn't read the pull requests. {{message}}", { message: list.error })
		: !list.data
			? t("Reading the pull requests")
			: t("No {{state}} pull requests", { state: state });
	return (
		<FlatList
			data={rows}
			keyExtractor={(row) => pullId(row.key)}
			contentContainerStyle={styles.content}
			refreshControl={refresh}
			ListHeaderComponent={
				<>
					{list.error && rows.length > 0 ? (
						<Text style={styles.failed} accessibilityRole="alert">
							{empty}
						</Text>
					) : null}
					{unknown ? (
						<View style={styles.banner}>
							<UnknownChecksBanner
								repo={unknown.key.repo}
								device={pullsDevice(view, project, now)}
							/>
						</View>
					) : null}
				</>
			}
			renderItem={({ item }) => (
				<PullRow
					row={item}
					device={pullDevice(view, item, project, now)}
					agents={agentsWithPull(view, item.key)}
					ago={formatAgoShort(item.pull.updated_at, now)}
					onOpen={onOpen}
					onAgent={onAgent}
				/>
			)}
			ListEmptyComponent={
				<Text
					style={[styles.note, list.error ? styles.error : null]}
					accessibilityRole={list.error ? "alert" : undefined}
				>
					{empty}
				</Text>
			}
		/>
	);
};

export const PullsScreen = ({ navigation, route }: ScreenProps<"Pulls">) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const projects = useMemo(() => pullProjects(view, now), [view, now]);
	const [picked, setPicked] = useState<ProjectId | undefined>(route.params?.project);
	const [state, setState] = useState<PullListState>("open");
	const [sort, setSort] = useState<PullSort>("ready");
	const project = picked ?? projects[0]?.id;
	const name = project ? (view.tables.project.get(project)?.name ?? t("Project")) : undefined;
	const others = projects.filter((p) => p.id !== project);
	const sortLabel = SORT_OPTIONS.find((option) => option.value === sort)?.label ?? "";

	const [onOpen] = useState(
		() => (row: PullSummary, device: DeviceId) =>
			navigation.navigate("Pull", { device, repo: row.key.repo, number: row.key.number })
	);
	const [onAgent] = useState(() => (agent: AgentId) => navigation.navigate("Agent", { agent }));
	const pickProject = () =>
		choose(
			t("Project"),
			projects.map((p) => ({
				label: p.name,
				disabled: p.id === project,
				pick: () => setPicked(p.id),
			}))
		);
	const pickSort = () =>
		choose(
			t("Sort: {{label}}", { label: sortLabel }),
			SORT_OPTIONS.map((option) => ({
				label: option.label,
				disabled: option.value === sort,
				pick: () => setSort(option.value),
			}))
		);

	return (
		<View style={styles.screen}>
			<Header
				title={t("Pull requests")}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
				subtitle={
					name === undefined ? undefined : others.length > 0 ? (
						<Pressable
							accessibilityRole="button"
							accessibilityLabel={t("Project")}
							accessibilityValue={{ text: name }}
							hitSlop={{ top: 8, bottom: 8 }}
							onPress={pickProject}
							style={styles.project}
						>
							<Text style={styles.sub} numberOfLines={1}>
								{name}
							</Text>
							<ChevronDown
								size={12}
								strokeWidth={2.2}
								color={color.mutedForeground}
							/>
						</Pressable>
					) : (
						<Text style={styles.sub} numberOfLines={1}>
							{name}
						</Text>
					)
				}
			/>
			{project ? (
				<>
					<View style={styles.bar}>
						<View style={styles.tabs}>
							<Tabs
								tabs={[
									{ value: "open", label: t("Open") },
									{ value: "closed", label: t("Closed") },
								]}
								value={state}
								onChange={setState}
							/>
						</View>
						<Pressable
							accessibilityRole="button"
							accessibilityLabel={t("Sort: {{label}}", { label: sortLabel })}
							onPress={pickSort}
							style={({ pressed }) => [styles.sort, pressed && styles.pressed]}
						>
							<ArrowUpDown
								size={13}
								strokeWidth={2}
								color={color.secondaryForeground}
							/>
							<Text style={styles.sortText} numberOfLines={1}>
								{sortLabel}
							</Text>
						</Pressable>
					</View>
					<PullList
						key={`${project}\n${state}`}
						project={project}
						state={state}
						sort={sort}
						onOpen={onOpen}
						onAgent={onAgent}
					/>
				</>
			) : (
				<Text style={styles.note}>
					{t(
						"No project shows pull requests yet. A project shows them once the GitHub App on its device reaches its repository."
					)}
				</Text>
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	project: { flexDirection: "row", alignItems: "center", gap: 4, alignSelf: "flex-start" },
	sub: { color: color.mutedForeground, fontSize: size.meta, flexShrink: 1 },
	bar: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 14,
		paddingTop: 8,
		paddingBottom: 4,
	},
	tabs: { flex: 1 },
	sort: {
		maxWidth: 150,
		minHeight: 40,
		flexDirection: "row",
		alignItems: "center",
		gap: 5,
		paddingHorizontal: 8,
		borderRadius: 6,
	},
	sortText: { color: color.secondaryForeground, fontSize: size.meta, flexShrink: 1 },
	content: { paddingHorizontal: 10, paddingTop: 6, paddingBottom: 32 },
	banner: { paddingHorizontal: 4, paddingBottom: 8 },
	item: { paddingVertical: 2 },
	row: { gap: 4, paddingHorizontal: 10, paddingVertical: 9, borderRadius: 6 },
	pressed: { backgroundColor: color.accent },
	top: { flexDirection: "row", alignItems: "flex-start", gap: 8 },
	number: {
		color: color.mutedForeground,
		fontFamily: font.mono,
		fontSize: size.meta,
		lineHeight: 19,
	},
	title: { flex: 1, color: color.foregroundStrong, fontSize: size.body, lineHeight: 19 },
	meta: { flexDirection: "row", alignItems: "center", gap: 8 },
	author: { flexShrink: 1, color: color.mutedForeground, fontSize: size.meta },
	ago: { marginLeft: "auto", color: color.mutedForeground, fontSize: size.meta },
	agents: {
		flexDirection: "row",
		flexWrap: "wrap",
		gap: 6,
		paddingHorizontal: 10,
		paddingBottom: 6,
	},
	agent: {
		maxWidth: 220,
		paddingHorizontal: 7,
		paddingVertical: 3,
		borderRadius: 5,
		backgroundColor: color.secondary,
	},
	agentText: { color: color.secondaryForeground, fontSize: size.tiny },
	note: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
	error: { color: color.errorText },
	failed: {
		color: color.errorText,
		fontSize: size.small,
		paddingHorizontal: 10,
		paddingBottom: 8,
	},
});
