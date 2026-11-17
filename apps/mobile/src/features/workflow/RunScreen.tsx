// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One workflow run, as the desktop's Workflow tab and canvas: its name,
// workflow and progress, the agent that leads it, the workflow's own
// header lines, and then its steps or its timeline. A step or a bar with
// an agent opens it. The ⋯ holds Stop run and Copy run ID.

import { useI18n } from "@maxhq/shared/i18n";

import { useCallback, useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { AgentId, NodeShape, NodeState, RunKey } from "@maxhq/shared/bindings";
import { formatNumber } from "@maxhq/shared/i18n";
import { useNow } from "@maxhq/shared/lib/clock";
import { badgeTone } from "@maxhq/shared/workflow/glyphs";
import { leadOf, runAt } from "@maxhq/shared/workflow/select";
import {
	useRunHeader,
	useRunLoad,
	useRunLoops,
	useRunNodes,
	useRunShape,
} from "@maxhq/shared/workflow/use-run";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size, TOUCH } from "../../theme";
import { choose } from "../../ui/choose";
import { ChevronRight } from "../../ui/glyphs";
import { Header, IconButton } from "../../ui/Header";
import { More } from "../../ui/Icon";
import { Tabs } from "../pulls/Tabs";
import { Badge } from "../test-hosts/parts";
import { NodeGlyph } from "./NodeGlyph";
import { openRunMenu } from "./RunMenu";
import { nodeSheet, TONE_TEXT } from "./runs";
import { RunSteps } from "./RunSteps";
import { RunTimeline } from "./RunTimeline";

const LINK = 36;
// Slop that makes the lead link a 44 point target.
const SLOP = { top: (TOUCH - LINK) / 2, bottom: (TOUCH - LINK) / 2 };

type Part = "steps" | "timeline";

/** The timeline, with its own clock so the steps don't redraw with it. */
const Timeline = (props: Omit<Parameters<typeof RunTimeline>[0], "now">) => {
	const now = useNow(5000);
	return <RunTimeline {...props} now={now} />;
};

export const RunScreen = ({ navigation, route }: ScreenProps<"Run">) => {
	const { t } = useI18n();
	const services = useServices();
	const { device, project, workflow, run: id } = route.params;
	const key = useMemo<RunKey>(
		() => ({ device, project, workflow, run: id }),
		[device, project, workflow, id]
	);
	const view = useView((s) => s.view);
	const run = useMemo(() => runAt(view, key), [view, key]);
	const lead = useMemo(() => leadOf(view, key), [view, key]);
	const leadTitle = lead === null ? undefined : view.tables.agent.get(lead)?.title;
	const shape = useRunShape(services.source, key);
	const nodes = useRunNodes(services.source, key);
	const loops = useRunLoops(services.source, key);
	const header = useRunHeader(services.source, key);
	const load = useRunLoad(services.source, key);
	const [part, setPart] = useState<Part>("steps");

	const openAgent = useCallback(
		(agent: AgentId) => navigation.navigate("Agent", { agent }),
		[navigation]
	);
	// A node's sheet: how it stands, its note, and an entry per agent that worked on it.
	const sheet = useCallback(
		(node: NodeShape, state: NodeState) => {
			const agents = services.view.getState().view.tables.agent;
			const { message, agents: found } = nodeSheet(
				state,
				(agent) => agents.get(agent)?.title
			);
			choose(
				node.label,
				found.map((agent) => ({ label: agent.title, pick: () => openAgent(agent.id) })),
				message
			);
		},
		[services, openAgent]
	);

	const progress = header?.progress ?? run?.progress;
	const lines = header?.lines ?? [];
	const orchestrator = run?.orchestrators.at(-1) ?? null;
	return (
		<View style={styles.screen}>
			<Header
				title={header?.label ?? run?.label ?? id}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
				subtitle={
					<View style={styles.subtitle}>
						<Text numberOfLines={1} style={styles.sub}>
							{workflow}
						</Text>
						{progress ? (
							<Text
								accessibilityLabel={t("{{done}} of {{total}} done", {
									done: formatNumber(progress.done),
									total: formatNumber(progress.total),
								})}
								style={[styles.sub, styles.progress]}
							>
								{formatNumber(progress.done)}/{formatNumber(progress.total)}
							</Text>
						) : null}
					</View>
				}
				right={
					run ? (
						<IconButton label={t("More")} onPress={() => openRunMenu(services, run)}>
							<More />
						</IconButton>
					) : null
				}
			/>
			<View style={styles.top}>
				{lead !== null && leadTitle !== undefined ? (
					<Pressable
						accessibilityRole="button"
						accessibilityLabel={`${t("Lead")}, ${leadTitle}`}
						accessibilityHint={t("Opens the agent")}
						hitSlop={SLOP}
						onPress={() => openAgent(lead)}
						style={({ pressed }) => [styles.lead, pressed && styles.pressed]}
					>
						{header ? <NodeGlyph status={header.status} decorative /> : null}
						<Text style={styles.leadKey}>{t("Lead")}</Text>
						<Text numberOfLines={1} style={styles.leadTitle}>
							{leadTitle}
						</Text>
						<ChevronRight size={14} strokeWidth={2} color={color.subtleForeground} />
					</Pressable>
				) : null}
				{lines.length > 0 ? (
					<View style={styles.lines}>
						{lines.map((line) => (
							<Badge key={line.text} label={line.text} tone={badgeTone(line.tone)} />
						))}
					</View>
				) : null}
				{load.notice ? (
					<Text
						accessibilityRole="alert"
						style={[styles.notice, { color: TONE_TEXT[load.notice.tone] }]}
					>
						{load.notice.text}
					</Text>
				) : null}
				<Tabs<Part>
					tabs={[
						{ value: "steps", label: t("Steps") },
						{ value: "timeline", label: t("Timeline") },
					]}
					value={part}
					onChange={setPart}
				/>
			</View>
			{shape ? (
				part === "steps" ? (
					<RunSteps
						// A new run starts from its own folds. A live run's shape grows
						// as it goes, and keeps its folds.
						key={run?.id ?? id}
						shape={shape}
						nodes={nodes}
						loops={loops}
						orchestrator={orchestrator}
						onOpenAgent={openAgent}
						onSheet={sheet}
					/>
				) : (
					<Timeline shape={shape} nodes={nodes} onOpenAgent={openAgent} />
				)
			) : load.status === "failed" ? (
				<Text accessibilityRole="alert" style={[styles.state, styles.failed]}>
					{t("Couldn't open the run{{value2}}", {
						value2: load.error ? `: ${load.error}` : "",
					})}
				</Text>
			) : (
				<Text style={styles.state}>{t("Loading the run")}</Text>
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	subtitle: { flexDirection: "row", alignItems: "center", gap: 8, minWidth: 0 },
	sub: { color: color.mutedForeground, fontSize: size.meta, flexShrink: 1 },
	progress: { flexShrink: 0, color: color.secondaryForeground, fontVariant: ["tabular-nums"] },
	top: { gap: 8, paddingHorizontal: 12, paddingTop: 8, paddingBottom: 6 },
	lead: {
		minHeight: LINK,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 6,
		borderRadius: 6,
	},
	pressed: { backgroundColor: color.accent },
	leadKey: { color: color.mutedForeground, fontSize: size.meta },
	leadTitle: { flex: 1, color: color.foreground, fontSize: size.small },
	lines: { flexDirection: "row", flexWrap: "wrap", gap: 4 },
	notice: { fontSize: size.tiny },
	state: { color: color.mutedForeground, fontSize: size.tiny, padding: 16 },
	failed: { color: color.errorText },
});
